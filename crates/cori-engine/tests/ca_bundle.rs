// SPDX-License-Identifier: Apache-2.0
//! `CORI_CA_FILE`: a client of `cori_engine::http` trusts a server whose certificate a private CA signed, which the
//! system's roots do not. The CA, the server's certificate and the server (`openssl s_server`) are made for the test; it
//! is skipped where the `openssl` command is missing. Its own test binary: the bundle is read once per process.
use std::process::{Command, Stdio};

fn openssl(args: &[&str], dir: &std::path::Path) -> bool {
    Command::new("openssl").args(args).current_dir(dir).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[tokio::test]
async fn a_private_ca_is_trusted_through_cori_ca_file() {
    let dir = std::env::temp_dir().join(format!("cori-ca-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("leaf.ext"), "subjectAltName=DNS:localhost,IP:127.0.0.1\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n").unwrap();
    let made = openssl(&["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key", "-out", "ca.pem", "-days", "2", "-subj", "/CN=cori test CA", "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign"], &dir)
        && openssl(&["req", "-newkey", "rsa:2048", "-nodes", "-keyout", "leaf.key", "-out", "leaf.csr", "-subj", "/CN=localhost"], &dir)
        && openssl(&["x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial", "-out", "leaf.pem", "-days", "2", "-extfile", "leaf.ext"], &dir);
    if !made { eprintln!("openssl not available: skipped"); return; }
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut server = Command::new("openssl").args(["s_server", "-quiet", "-www", "-accept", &port.to_string(), "-cert", "leaf.pem", "-key", "leaf.key"]).current_dir(&dir)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let url = format!("https://localhost:{port}/");
    // the server takes a moment to listen
    for _ in 0..50 { if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() { break; } std::thread::sleep(std::time::Duration::from_millis(100)); }
    let without = reqwest::Client::new().get(&url).send().await;
    assert!(without.is_err(), "the system's roots alone must not trust the test CA");
    unsafe { std::env::set_var("CORI_CA_FILE", dir.join("ca.pem")); }
    assert_eq!(cori_engine::http::check(), Ok(1));
    let with = cori_engine::http::builder().build().unwrap().get(&url).send().await;
    let _ = server.kill();
    let r = with.expect("trusted through CORI_CA_FILE");
    assert!(r.status().is_success(), "{}", r.status());
    let _ = std::fs::remove_dir_all(&dir);
}
