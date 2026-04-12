use std::io;
use std::os::fd::OwnedFd;

use bento_plugins::{
    emit_event, into_async_stream, read_startup_message, recv_conn_fd, PluginEvent,
};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;

#[tokio::main]
async fn main() -> io::Result<()> {
    let startup = match read_startup_message() {
        Ok(startup) => startup,
        Err(err) => {
            let message = format!("read startup message: {err}");
            let _ = emit_event(PluginEvent::Failed { message: &message });
            return Err(err);
        }
    };

    if let Err(err) = startup.expect_listen() {
        let message = format!(
            "invalid startup message for {}:{}: {err}",
            startup.endpoint, startup.port
        );
        let _ = emit_event(PluginEvent::Failed { message: &message });
        return Err(err);
    }

    emit_event(PluginEvent::Ready)?;
    emit_event(PluginEvent::EndpointStatus {
        active: true,
        summary: "hello-world ready",
        problems: &[],
    })?;

    loop {
        let (conn_fd, _conn_id) = recv_conn_fd(startup.fd)?;
        tokio::spawn(async move {
            if let Err(err) = handle_connection(conn_fd).await {
                eprintln!("hello-world plugin connection failed: {err}");
            }
        });
    }
}

async fn handle_connection(conn_fd: OwnedFd) -> io::Result<()> {
    let stream = into_async_stream(conn_fd)?;
    let io = TokioIo::new(stream);
    http1::Builder::new()
        .serve_connection(io, service_fn(handle_request))
        .await
        .map_err(io::Error::other)
}

async fn handle_request(
    request: Request<Incoming>,
) -> Result<Response<Full<Bytes>>, std::convert::Infallible> {
    let response = if request.method() == Method::GET {
        Response::builder()
            .status(StatusCode::OK)
            .header(hyper::header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from_static(b"hello world\n")))
            .expect("response should build")
    } else {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(hyper::header::CONTENT_TYPE, "text/plain")
            .body(Full::new(Bytes::from_static(b"not found\n")))
            .expect("response should build")
    };

    Ok(response)
}
