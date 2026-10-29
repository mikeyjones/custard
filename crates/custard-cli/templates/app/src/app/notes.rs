//! `/notes`: each signed-in person's own notes, kept in the database.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, route,
    },
    view::{View, attributes, view},
};

use crate::{
    components::{button::button, input::input},
    features::notes,
};

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let user = custard::auth::require_user(cx).await?;
    let notes = notes::for_user(custard::db(cx), &user.user_id).await?;

    Ok(view! {
        <main class="mx-auto max-w-3xl space-y-6 px-4 py-12">
            <h1 class="text-2xl font-semibold">"Notes"</h1>
            <form method="post" action=(href!(add)) class="flex gap-2">
                input(
                    attrs: attributes! {
                        name="body"
                        placeholder="Write a note"
                        required=""
                        autocomplete="off"
                    }
                )
                button(attrs: attributes! { type="submit" }, "Add")
            </form>
            if notes.is_empty() {
                <p class="text-muted-foreground">"No notes yet."</p>
            } else {
                <ul class="space-y-2">
                    for note in notes {
                        <li
                            id=(format!("note-{}", note.id))
                            class="rounded-md border px-3 py-2"
                        >
                            <p>(note.body)</p>
                            <p class="text-xs text-muted-foreground">
                                (note.created_at.format("%-d %b %Y, %H:%M").to_string())
                            </p>
                        </li>
                    }
                </ul>
            }
        </main>
    })
}

#[derive(Deserialize)]
pub struct NewNote {
    body: String,
}

#[route(POST)]
pub async fn add(cx: &Cx, Form(new_note): Form<NewNote>) -> Result<SeeOther> {
    let user = custard::auth::require_user(cx).await?;
    let body = new_note.body.trim();
    if !body.is_empty() {
        notes::add(custard::db(cx), &user.user_id, body).await?;
    }
    Ok(see_other(href!(page).resolve(cx)))
}
