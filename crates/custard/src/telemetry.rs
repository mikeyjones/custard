//! Logs, traces and errors.
//!
//! Everything is logged with [`tracing`] to standard output, which The Custard
//! Factory keeps with no SDK at all. When the platform sets
//! `OTEL_EXPORTER_OTLP_ENDPOINT`, traces and logs are also sent over OTLP,
//! with the endpoint, headers and resource the `OTEL_*` variables give.
//!
//! Every request is a span with the attributes the platform's issue tracker
//! reads (`http.route`, `url.path`, `http.response.status_code`,
//! `user_agent.original`). A request that fails with a 500, and any panic,
//! is recorded as an `exception` event with `exception.type`,
//! `exception.message` and `exception.stacktrace`, which the platform turns
//! into an issue.

use std::backtrace::Backtrace;

use opentelemetry::{KeyValue, global, propagation::Extractor, trace::TracerProvider as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, SpanExporter};
use opentelemetry_sdk::{
    Resource, logs::SdkLoggerProvider, propagation::TraceContextPropagator,
    trace::SdkTracerProvider,
};
use topcoat::{
    context::Cx,
    router::{
        Body, HeaderMap, Layer, LayerFuture, Next, Path,
        error::{
            BadRequestError, ContentTooLargeError, ForbiddenError, MethodNotAllowedError,
            NotFoundError, RedirectError, SeeOther, ServiceUnavailableError, TooManyRequestsError,
            UnauthorizedError,
        },
        request, try_endpoint,
    },
};
use tracing::{Instrument as _, field::Empty};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

/// Sends what is left when dropped, as the app stops.
pub struct Telemetry {
    tracer: Option<SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        let tracer = self.tracer.take();
        let logger = self.logger.take();
        off_runtime(move || {
            if let Some(tracer) = tracer {
                let _ = tracer.shutdown();
            }
            if let Some(logger) = logger {
                let _ = logger.shutdown();
            }
        });
    }
}

/// Runs `work` on a thread outside the async runtime, and waits for it.
fn off_runtime<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::spawn(work)
        .join()
        .expect("telemetry setup does not panic")
}

/// What to log, unless `RUST_LOG` says otherwise.
const DEFAULT_FILTER: &str = "info,sqlx=warn,hyper=warn,reqwest=warn";

/// Starts logging, and sending telemetry when `export` is set. Call once,
/// before anything logs.
pub fn init(name: &str, version: &str, export: bool) -> Telemetry {
    let filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    let fmt = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_filter(filter());
    install_panic_hook();

    if !export {
        tracing_subscriber::registry().with(fmt).init();
        return Telemetry {
            tracer: None,
            logger: None,
        };
    }

    let mut resource =
        Resource::builder().with_attribute(KeyValue::new("service.version", version.to_owned()));
    if std::env::var_os("OTEL_SERVICE_NAME").is_none() {
        resource = resource.with_service_name(name.to_owned());
    }
    let resource = resource.build();

    // The exporters send from threads of their own with a blocking HTTP client,
    // which must not be made, or shut down, on the async runtime's threads.
    let (tracer, logger) = off_runtime(move || {
        let tracer = SpanExporter::builder().with_http().build().map(|exporter| {
            SdkTracerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource.clone())
                .build()
        });
        let logger = LogExporter::builder().with_http().build().map(|exporter| {
            SdkLoggerProvider::builder()
                .with_batch_exporter(exporter)
                .with_resource(resource)
                .build()
        });
        (tracer, logger)
    });

    global::set_text_map_propagator(TraceContextPropagator::new());

    // The exporters' own HTTP clients log too: never send those, or every
    // export would make another.
    let quiet = || {
        filter()
            .add_directive("hyper=off".parse().unwrap())
            .add_directive("reqwest=off".parse().unwrap())
            .add_directive("opentelemetry=off".parse().unwrap())
            .add_directive("opentelemetry_sdk=off".parse().unwrap())
    };

    let (tracer, trace_layer) = match tracer {
        Ok(provider) => {
            let layer = tracing_opentelemetry::layer()
                .with_tracer(provider.tracer("custard"))
                .with_filter(quiet());
            (Some(provider), Some(layer))
        }
        Err(error) => {
            eprintln!("custard: traces will not be sent: {error}");
            (None, None)
        }
    };
    let (logger, log_layer) = match logger {
        Ok(provider) => {
            let layer = OpenTelemetryTracingBridge::new(&provider).with_filter(quiet());
            (Some(provider), Some(layer))
        }
        Err(error) => {
            eprintln!("custard: logs will not be sent: {error}");
            (None, None)
        }
    };

    tracing_subscriber::registry()
        .with(fmt)
        .with(trace_layer)
        .with(log_layer)
        .init();
    Telemetry { tracer, logger }
}

/// Records a panic as an exception, in the request it happened in.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = match info.payload().downcast_ref::<&str>() {
            Some(text) => (*text).to_owned(),
            None => info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "panic".into()),
        };
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        tracing::error!(
            exception.r#type = "panic",
            exception.message = %format!("{message}{location}"),
            exception.stacktrace = %Backtrace::force_capture(),
            "exception"
        );
        previous(info);
    }));
}

/// Wraps every request, including ones that match no route, in a span.
pub(crate) struct RequestTelemetry;

impl Layer for RequestTelemetry {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        let method = request::method(cx).as_str().to_owned();
        let path = request::uri(cx).path().to_owned();
        let route = try_endpoint(cx).map(|endpoint| endpoint.path().to_string());
        let user_agent = request::headers(cx)
            .get("user-agent")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let span = tracing::info_span!(
            "request",
            otel.name = %format!("{method} {}", route.as_deref().unwrap_or(&path)),
            otel.kind = "server",
            otel.status_code = Empty,
            http.request.method = %method,
            http.route = route.as_deref(),
            url.path = %path,
            user_agent.original = %user_agent,
            http.response.status_code = Empty,
        );
        let parent = global::get_text_map_propagator(|propagator| {
            propagator.extract(&Headers(request::headers(cx)))
        });
        let _ = span.set_parent(parent);

        Box::pin(
            async move {
                let result = next.run(cx, body).await;
                let span = tracing::Span::current();
                let status = match &result {
                    Ok(response) => Some(response.status().as_u16()),
                    Err(error) => status_of(error),
                };
                if let Some(status) = status {
                    span.record("http.response.status_code", status);
                    if status >= 500 {
                        span.record("otel.status_code", "ERROR");
                    }
                }
                if let Err(error) = &result
                    && status == Some(500)
                {
                    tracing::error!(
                        exception.r#type = "topcoat::Error",
                        exception.message = %error,
                        exception.stacktrace = %format!("{error:?}"),
                        "exception"
                    );
                }
                result
            }
            .instrument(span),
        )
    }
}

/// The status the router will answer an error with; `None` for a rewrite,
/// which is answered by another route.
fn status_of(error: &topcoat::Error) -> Option<u16> {
    macro_rules! status {
        ($($ty:ty => $status:expr),* $(,)?) => {
            $(if error.downcast_ref::<$ty>().is_some() { return Some($status); })*
        };
    }
    status!(
        NotFoundError => 404,
        UnauthorizedError => 401,
        ForbiddenError => 403,
        BadRequestError => 400,
        ContentTooLargeError => 413,
        MethodNotAllowedError => 405,
        RedirectError => 307,
        SeeOther => 303,
        ServiceUnavailableError => 503,
        TooManyRequestsError => 429,
    );
    if error
        .downcast_ref::<topcoat::router::error::RewriteError>()
        .is_some()
    {
        return None;
    }
    Some(500)
}

struct Headers<'a>(&'a HeaderMap);

impl Extractor for Headers<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}
