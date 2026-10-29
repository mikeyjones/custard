mod account;
mod notes;

use custard::{Platform, RouterBuilderCustardExt};
use topcoat::{
    Result,
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::{Cx, app_context},
    font::{Font, RouterBuilderFontExt, fontsource::fontsource_font},
    router::{RouterBuilder, Slot, href, layout, module_router, page},
    runtime::RouterBuilderRuntimeExt,
    tailwind,
    view::{View, view},
};

use crate::components::{
    badge::{BadgeVariant, badge},
    button::{ButtonSize, ButtonVariant, button_variants},
    card::{card, card_content, card_description, card_header, card_title},
};

/// The Geist font family, loaded from Fontsource.
const GEIST: Font = fontsource_font!(GEIST, weight: [400, 700], style: Normal);

/// Builds the application's router.
pub fn router(platform: &Platform) -> RouterBuilder {
    // The pages and layouts of this module and its submodules, with paths derived
    // from the module tree.
    module_router!()
        .font(GEIST)
        // The database, sign-in, file storage and telemetry The Custard Factory gives the app.
        .custard(platform)
        .assets(AssetBundle::load().unwrap())
        // Lets the browser runtime rerun pages; register it after other layers.
        .runtime()
}

#[layout]
async fn root(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    let signed_in = custard::auth::user(cx).await.map(|user| {
        if user.name.is_empty() {
            user.email.clone()
        } else {
            user.name.clone()
        }
    });

    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"{{title}}"</title>
                <link rel="stylesheet" href=(tailwind::stylesheet!())>
                topcoat::font::link(font: GEIST)
                topcoat::runtime::script()
                topcoat::dev::script()
            </head>
            <body>
                <header class="border-b">
                    <nav class="mx-auto flex max-w-3xl items-center gap-6 px-4 py-3">
                        <a href=(href!(home)) class="font-semibold">"{{title}}"</a>
                        <a
                            href=(href!(notes::page))
                            class="text-sm text-muted-foreground hover:text-foreground"
                        >
                            "Notes"
                        </a>
                        <div class="ml-auto flex items-center gap-3 text-sm">
                            match signed_in {
                                Some(name) => {
                                    <a href=(href!(account::page)) class="hover:underline">
                                        (name)
                                    </a>
                                    <a
                                        href=(custard::auth::LOGOUT_PATH)
                                        class=(button_variants(
                                            ButtonVariant::Outline,
                                            ButtonSize::Sm,
                                        ))
                                    >
                                        "Sign out"
                                    </a>
                                }
                                None => {
                                    <a
                                        href=(custard::auth::LOGIN_PATH)
                                        class=(button_variants(
                                            ButtonVariant::Primary,
                                            ButtonSize::Sm,
                                        ))
                                    >
                                        "Sign in"
                                    </a>
                                }
                            }
                        </div>
                    </nav>
                </header>
                (slot)
            </body>
        </html>
    })
}

#[page]
async fn home(cx: &Cx) -> Result<impl View> {
    let env = app_context::<custard::Env>(cx);
    let services = [
        ("Database", custard::try_db(cx).is_some(), "DATABASE_URL"),
        (
            "Sign-in",
            custard::auth::user(cx).await.is_some() || env.auth.is_some(),
            "CUSTARD_AUTH_*",
        ),
        ("File storage", custard::try_storage(cx).is_some(), "S3_*"),
        ("Telemetry", env.telemetry, "OTEL_EXPORTER_OTLP_ENDPOINT"),
    ];

    Ok(view! {
        <main class="mx-auto max-w-3xl px-4 py-12">
            card(
                card_header(
                    card_title("{{title}}")
                    card_description(
                        "A Topcoat app on The Custard Factory. Edit "
                        <code>"src/app.rs"</code>
                        " and save to see your changes."
                    )
                )
                card_content(
                    <ul class="divide-y">
                        for (service, connected, variables) in services {
                            <li class="flex items-center justify-between py-2">
                                <span>(service)</span>
                                if connected {
                                    badge("Connected")
                                } else {
                                    badge(
                                        variant: BadgeVariant::Outline,
                                        "Set "
                                        (variables)
                                    )
                                }
                            </li>
                        }
                    </ul>
                )
            )
        </main>
    })
}
