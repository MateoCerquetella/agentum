CREATE TABLE sdd_integration_connections (
    connection_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    external_account_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    selected_site_id TEXT,
    metadata_json TEXT NOT NULL,
    credential_revision INTEGER NOT NULL CHECK(credential_revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(provider, external_account_id)
);
CREATE INDEX idx_sdd_integration_connections_provider
    ON sdd_integration_connections(provider, updated_at DESC);

CREATE TABLE sdd_oauth_flows (
    flow_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    request_id TEXT NOT NULL UNIQUE,
    state_hash TEXT NOT NULL UNIQUE,
    redemption_id TEXT NOT NULL UNIQUE,
    authorization_url TEXT NOT NULL,
    device_key_ref TEXT NOT NULL,
    connection_id TEXT,
    status TEXT NOT NULL CHECK(status IN (
        'pending', 'redeeming', 'sync_pending', 'redeemed', 'failed', 'expired'
    )),
    revision INTEGER NOT NULL CHECK(revision > 0),
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    redeemed_at TEXT
);
CREATE INDEX idx_sdd_oauth_flows_status_expiry
    ON sdd_oauth_flows(provider, status, expires_at);
