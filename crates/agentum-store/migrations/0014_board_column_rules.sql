-- Per-server override of the compile-time required-field matrix from
-- slice 1 (`agentum-core::board_schema::required_fields_for`). A row
-- here replaces the const default for that column; an absent row means
-- "use the const" for `todo`/`doing`/`done` and "passthrough" for any
-- other column.
--
-- `required_fields` is a JSON array of wire-vocabulary strings (e.g.
-- `["title","lbl","workdir"]`) — same identifiers
-- `RequiredField::as_missing_key` produces. JSON-blob over a normalised
-- two-column table because rules are read as a complete set per column;
-- no slice 2 query looks up by individual field. Denormalisation costs
-- nothing and keeps the upsert a single statement.
--
-- `updated_at` is the only audit affordance in slice 2 — deliberately
-- not exposed via `GET /api/board/rules` to keep the wire shape minimal
-- until a dedicated audit endpoint lands.

CREATE TABLE board_column_rules (
    column_name      TEXT PRIMARY KEY,
    required_fields  TEXT NOT NULL,
    updated_at       TEXT NOT NULL
);
