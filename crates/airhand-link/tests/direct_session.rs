//! In-process QUIC, without Tailcat or the broker.
use airhand_link::direct::{
    Secret,
    tls::{self, Identity},
};
use airhand_link::transport::{accept_session_with_timeout, open_session};
use airhand_proto::{Role, SessionHeader, SessionLane};
use quinn::{Connection, Endpoint};
use std::time::Duration;

fn identity() -> Identity {
    Identity {
        certificate: include_bytes!("direct_quinn/fixtures/cert.der").to_vec(),
        private_key: Secret::new(include_bytes!("direct_quinn/fixtures/key.der").to_vec()),
    }
}
async fn pair() -> (Endpoint, Endpoint, Connection, Connection) {
    let id = identity();
    let host = Endpoint::server(
        tls::server_config(&id, id.fingerprint()).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let mut ctrl = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    ctrl.set_default_client_config(tls::client_config(&id, id.fingerprint()).unwrap());
    let client = ctrl.connect(host.local_addr().unwrap(), "airhand").unwrap();
    let server = async { host.accept().await.unwrap().await.unwrap() };
    let (client, server) = tokio::join!(client, server);
    (host, ctrl, server, client.unwrap())
}
async fn header(conn: &Connection, id: u64, lane: SessionLane) {
    let (mut tx, _rx) = conn.open_bi().await.unwrap();
    tx.write_all(
        &SessionHeader {
            session_id: id,
            role: Role::Controller,
            lane,
        }
        .encode(),
    )
    .await
    .unwrap();
    tx.finish().unwrap();
}
#[tokio::test]
async fn accepts_three_lanes_and_rejects_a_fourth() {
    let (_host, _ctrl, server, client) = pair().await;
    let receive = accept_session_with_timeout(&server, Duration::from_secs(1));
    let (session, streams) = tokio::join!(receive, open_session(&client, 42, Role::Controller));
    let session = session.unwrap();
    let _streams = streams.unwrap();
    assert_eq!(session.session_id, 42);
    header(&client, 42, SessionLane::Control).await;
    assert!(
        tokio::time::timeout(Duration::from_secs(1), server.closed())
            .await
            .is_ok()
    );
    drop(session);
}
#[tokio::test]
async fn duplicate_lane_and_foreign_session_close_connection() {
    for foreign in [false, true] {
        let (_host, _ctrl, server, client) = pair().await;
        let send = async {
            header(&client, 42, SessionLane::Control).await;
            header(
                &client,
                if foreign { 43 } else { 42 },
                if foreign {
                    SessionLane::Video
                } else {
                    SessionLane::Control
                },
            )
            .await;
        };
        let (received, _) = tokio::join!(
            accept_session_with_timeout(&server, Duration::from_secs(1)),
            send
        );
        assert!(received.is_err());
        assert!(server.close_reason().is_some());
    }
}
#[tokio::test]
async fn partial_header_shares_the_session_deadline() {
    let (_host, _ctrl, server, client) = pair().await;
    let (mut send, _) = client.open_bi().await.unwrap();
    send.write_all(b"A").await.unwrap();
    assert!(
        accept_session_with_timeout(&server, Duration::from_millis(30))
            .await
            .is_err()
    );
    assert!(server.close_reason().is_some());
}
#[tokio::test]
async fn rejects_wrong_server_and_client_pins() {
    for wrong_server in [false, true] {
        let id = identity();
        let wrong = [99; 32];
        let host = Endpoint::server(
            tls::server_config(
                &id,
                if wrong_server {
                    id.fingerprint()
                } else {
                    wrong
                },
            )
            .unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        )
        .unwrap();
        let mut ctrl = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        ctrl.set_default_client_config(
            tls::client_config(
                &id,
                if wrong_server {
                    wrong
                } else {
                    id.fingerprint()
                },
            )
            .unwrap(),
        );
        let client = ctrl.connect(host.local_addr().unwrap(), "airhand").unwrap();
        let server = async { host.accept().await.unwrap().await };
        let (client, server) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(client, server)
        })
        .await
        .unwrap();
        assert!(server.is_err());
        if let Ok(client) = client {
            assert!(
                tokio::time::timeout(Duration::from_secs(1), client.closed())
                    .await
                    .is_ok()
            );
        }
    }
}
#[test]
fn secret_debug_is_redacted() {
    assert_eq!(
        format!("{:?}", Secret::new("sensitive-example")),
        "[REDACTED]"
    );
}

#[tokio::test]
async fn denied_direct_consent_never_exposes_host_hello_or_media() {
    use airhand_link::controller::{
        ControllerService, DirectSession, Frontend, SessionEvent, SessionUi, UiClosed,
    };
    use airhand_link::direct::pairing::DirectHello;
    use airhand_proto::framing::read_msg;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Ui(AtomicUsize);
    impl SessionUi for Ui {
        fn packet(&self, _: Vec<u8>) -> Result<(), UiClosed> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn event(&self, event: SessionEvent) -> Result<(), UiClosed> {
            if matches!(event, SessionEvent::Hello { .. }) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
    }
    let (_host, _ctrl, server, client) = pair().await;
    let service = ControllerService::start_direct(tokio::runtime::Handle::current());
    let ui = Arc::new(Ui(AtomicUsize::new(0)));
    let (_gate, path) = tokio::sync::watch::channel(true);
    let deny = async {
        let mut session = accept_session_with_timeout(&server, Duration::from_secs(1))
            .await
            .unwrap();
        let hello =
            read_msg::<_, DirectHello>(&mut session.streams.control.1, 1024, &mut Vec::new())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(hello.v, 1);
        server.close(0u32.into(), b"declined");
    };
    let connect = service.connect_direct(
        &client,
        DirectSession {
            path,
            nonce: Secret::new([7; 32]),
            sas: Some("123456".into()),
        },
        Default::default(),
        ui.clone(),
        Frontend::Browser,
    );
    let (result, _) = tokio::join!(connect, deny);
    assert!(result.is_err());
    assert_eq!(ui.0.load(Ordering::Relaxed), 0);
}

#[test]
fn ed25519_seed_pkcs8_is_supported() {
    let identity = Identity::from_ed25519_seed(Vec::new(), &Secret::new([7; 32]));
    let key =
        rustls::pki_types::PrivatePkcs8KeyDer::from(identity.private_key.expose().clone()).into();
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
    assert!(
        signer
            .choose_scheme(&[rustls::SignatureScheme::ED25519])
            .is_some()
    );
}

#[tokio::test]
#[ignore = "requires a built helper via AIRHAND_HELPER_TEST_BINARY"]
async fn helper_prepared_ed25519_identity_is_usable_for_mtls() {
    use std::{
        io::{BufRead, BufReader, Write},
        process::{Command, Stdio},
    };
    let binary =
        std::env::var_os("AIRHAND_HELPER_TEST_BINARY").expect("build and select the helper first");
    let seed: [u8; 32] = rand::random();
    let node: [u8; 32] = rand::random();
    let node = format!(
        "privkey:{}",
        node.iter().map(|b| format!("{b:02x}")).collect::<String>()
    );
    let mut helper = Command::new(binary)
        .arg("--control")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let config = serde_json::json!({"cmd":"prepare","mode":"controller","node_private":node,"tls_seed":seed});
    writeln!(helper.stdin.as_mut().unwrap(), "{}", config).unwrap();
    let mut response = String::new();
    BufReader::new(helper.stdout.take().unwrap())
        .read_line(&mut response)
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    let certificate = serde_json::from_value(response["certificate"].clone()).unwrap();
    drop(helper.stdin.take());
    assert!(helper.wait().unwrap().success());
    let identity = Identity::from_ed25519_seed(certificate, &Secret::new(seed));
    let server = Endpoint::server(
        tls::server_config(&identity, identity.fingerprint()).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let mut client = Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client
        .set_default_client_config(tls::client_config(&identity, identity.fingerprint()).unwrap());
    let connecting = client
        .connect(server.local_addr().unwrap(), "airhand")
        .unwrap();
    let accept = async { server.accept().await.unwrap().await };
    let (client, host) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(connecting, accept)
    })
    .await
    .unwrap();
    assert!(client.is_ok() && host.is_ok());
}
