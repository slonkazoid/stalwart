/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::time::{Duration, Instant};

use common::config::server::ServerProtocol;
use mail_auth::MX;
use store::write::now;

use crate::smtp::{DnsCache, TestSMTP, session::TestSession};

const LOCAL: &str = r#"
[queue.strategy]
route = [{if = "retry_num > 0", then = "'fallback'"},
            {else = "'mx'"}]

[session.rcpt]
relay = true
max-recipients = 100

[session.extensions]
dsn = true

[queue.route.fallback]
type = "relay"
address = fallback.foobar.org
port = 9925
protocol = 'smtp'
concurrency = 5

[queue.route.fallback.tls]
implicit = false
allow-invalid-certs = true

"#;

const REMOTE: &str = r#"
[session.rcpt]
relay = true

[session.ehlo]
reject-non-fqdn = false

[session.extensions]
dsn = true
chunking = false
"#;

#[tokio::test]
#[serial_test::serial]
async fn fallback_relay() {
    // Enable logging
    crate::enable_logging();

    // Start test server
    let mut remote = TestSMTP::new("smtp_fallback_remote", REMOTE).await;
    let _rx = remote.start(&[ServerProtocol::Smtp]).await;
    let mut local = TestSMTP::new("smtp_fallback_local", LOCAL).await;

    // Add mock DNS entries
    let core = local.build_smtp();
    core.mx_add(
        "foobar.org",
        vec![MX {
            exchanges: vec!["_dns_error.foobar.org".to_string()],
            preference: 10,
        }],
        Instant::now() + Duration::from_secs(10),
    );
    /*core.ipv4_add(
        "unreachable.foobar.org",
        vec!["127.0.0.2".parse().unwrap()],
        Instant::now() + Duration::from_secs(10),
    );*/
    core.ipv4_add(
        "fallback.foobar.org",
        vec!["127.0.0.1".parse().unwrap()],
        Instant::now() + Duration::from_secs(10),
    );

    let mut session = local.new_session();
    session.data.remote_ip_str = "10.0.0.1".into();
    session.eval_session_params().await;
    session.ehlo("mx.test.org").await;
    session
        .send_message("john@test.org", &["bill@foobar.org"], "test:no_dkim", "250")
        .await;
    local
        .queue_receiver
        .expect_message_then_deliver()
        .await
        .try_deliver(core.clone());
    let mut retry = local.queue_receiver.expect_message().await;
    let prev_due = retry.message.recipients[0].retry.due;
    let next_due = now();
    let queue_id = retry.queue_id;
    retry.message.recipients[0].retry.due = next_due;
    retry.save_changes(&core, prev_due.into()).await;
    local
        .queue_receiver
        .delivery_attempt(queue_id)
        .await
        .try_deliver(core.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    remote.queue_receiver.expect_message().await;
}
