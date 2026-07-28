# syntax=docker/dockerfile:1.7

FROM rust:1.94.1-bookworm@sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55 AS build
WORKDIR /source
COPY . .
RUN cargo build --locked --release -p agentum-jira-broker

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 agentum-broker \
    && useradd --uid 10001 --gid agentum-broker --no-create-home --shell /usr/sbin/nologin agentum-broker \
    && install -d -o agentum-broker -g agentum-broker -m 0700 /var/lib/agentum-jira-broker

COPY --from=build /source/target/release/agentum-jira-broker /usr/local/bin/agentum-jira-broker

USER 10001:10001
ENV AGENTUM_JIRA_BROKER_BIND=0.0.0.0:8787 \
    AGENTUM_JIRA_BROKER_TRUST_PROXY_TLS=true \
    AGENTUM_JIRA_BROKER_DB=/var/lib/agentum-jira-broker/broker.sqlite3 \
    RUST_LOG=agentum_jira_broker=info
EXPOSE 8787
VOLUME ["/var/lib/agentum-jira-broker"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["curl", "--fail", "--silent", "--show-error", "--max-time", "2", "http://127.0.0.1:8787/healthz"]
ENTRYPOINT ["/usr/local/bin/agentum-jira-broker"]
