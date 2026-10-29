//! `/account`: who is signed in, as the sign-in service says.

use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, view},
};

use crate::components::{
    badge::{BadgeVariant, badge},
    card::{card, card_content, card_description, card_header, card_title},
};

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let user = custard::auth::require_user(cx).await?.clone();
    let details = [
        ("User ID", user.user_id),
        ("Email", user.email),
        ("Organisation", user.org_name),
    ];

    Ok(view! {
        <main class="mx-auto max-w-3xl px-4 py-12">
            card(
                card_header(
                    card_title("Your account")
                    card_description("From the environment's sign-in service.")
                )
                card_content(
                    <dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
                        for (label, value) in details {
                            <dt class="text-muted-foreground">(label)</dt>
                            <dd>(value)</dd>
                        }
                        <dt class="text-muted-foreground">"Roles"</dt>
                        <dd class="flex flex-wrap gap-1">
                            if user.roles.is_empty() {
                                badge(variant: BadgeVariant::Outline, "None")
                            }
                            for role in user.roles {
                                badge(variant: BadgeVariant::Secondary, (role))
                            }
                        </dd>
                    </dl>
                )
            )
        </main>
    })
}
