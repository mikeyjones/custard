CREATE TABLE notes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    -- The `user_id` from the sign-in service: stable for the life of the environment.
    user_id TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX notes_by_user ON notes (user_id, created_at DESC);
