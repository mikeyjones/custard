//! Notes: what each person has written, by their `user_id` from the sign-in
//! service. The table is made in `migrations/`.

use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

#[derive(Clone, Debug, FromRow)]
pub struct Note {
    pub id: i64,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

/// Someone's notes, newest first.
pub async fn for_user(db: &PgPool, user_id: &str) -> sqlx::Result<Vec<Note>> {
    sqlx::query_as(
        "SELECT id, body, created_at FROM notes WHERE user_id = $1 ORDER BY created_at DESC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await
}

pub async fn add(db: &PgPool, user_id: &str, body: &str) -> sqlx::Result<Note> {
    sqlx::query_as(
        "INSERT INTO notes (user_id, body) VALUES ($1, $2) RETURNING id, body, created_at",
    )
    .bind(user_id)
    .bind(body)
    .fetch_one(db)
    .await
}
