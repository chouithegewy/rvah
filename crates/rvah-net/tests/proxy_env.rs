//! Proxy variables in the environment must not re-route add-on traffic:
//! a proxy would resolve the host itself, past the vetted, pinned addresses.
//! Its own test binary, since the environment is process-wide.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};

use rvah_addon_sdk::HttpRequest;
use rvah_net::{HttpTransport, Transport};

#[test]
fn proxy_variables_do_not_reroute_pinned_requests() {
    // A proxy nobody listens on: any request sent through it fails.
    for name in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "ALL_PROXY"] {
        unsafe { std::env::set_var(name, "http://127.0.0.1:9") };
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0u8; 4096];
        let _ = stream.read(&mut buf).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\npinned")
            .unwrap();
    });
    let req = HttpRequest::get(format!("http://pinned.invalid:{port}/"));
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let resp = HttpTransport::default()
        .execute_pinned(&req, &[addr])
        .unwrap();
    assert_eq!(resp.body, b"pinned");
    server.join().unwrap();
}
