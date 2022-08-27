use std::convert::Infallible;
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use hyper::server::conn::AddrStream;
use hyper::service::{make_service_fn, service_fn};
use hyper::Server;
use hyper::{Body, Request, Response};

use futures::Future;

use log::{debug, error, info};

extern crate serde_json;

pub mod http;

/// Errors that can be returned by the function handler
pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A type for handling incoming requests and producing responses
pub trait Handler<Req, Resp> {
    /// Errors returned by the handler
    type Error;
    /// Future type returned by the handler
    type Fut: Future<Output = Result<Resp, Self::Error>>;
    /// Handle the incoming request
    fn call(self, req: Req) -> Self::Fut;
}

/// [`Handler`] implementation for `FnOnce`
impl<Req, Resp, Error, Fut, F> Handler<Req, Resp> for F
where
    F: FnOnce(Req) -> Fut,
    Fut: Future<Output = Result<Resp, Error>>,
    Error: Into<crate::Error>,
{
    type Error = Error;
    type Fut = Fut;
    fn call(self, req: Req) -> Self::Fut {
        (self)(req)
    }
}

pub async fn run<Req, Resp, F>(handler: F) -> Result<(), Error>
where
    F: Handler<Req, Resp> + Clone + Send + Sync + 'static,
    <F as Handler<Req, Resp>>::Error: std::fmt::Display,
    <F as Handler<Req, Resp>>::Fut: Send,
    Req: for<'de> Deserialize<'de> + Send,
    Resp: Serialize,
    Error: Into<crate::Error> + Send,
{
    let make_service = make_service_fn(move |conn: &AddrStream| {
        let client_addr = conn.remote_addr();

        let handler = handler.clone();
        let service = service_fn(move |req: Request<Body>| {
            let handler = handler.clone();
            async move {
                debug!("New request from {}", client_addr);

                // Transoform `Body` into `Bytes`
                let body = req.into_body();
                let bytes = match hyper::body::to_bytes(body).await {
                    Ok(bytes) => bytes,
                    Err(err) => {
                        error!("Could not parse body");
                        return Ok::<_, Infallible>(Response::new(Body::from(format!(
                            "Runtime error: {}",
                            err
                        ))));
                    }
                };

                // Deserialize received bytes using serde_json
                let request = match serde_json::from_slice(&bytes) {
                    Ok(request) => request,
                    Err(err) => {
                        error!("Could not de-serialize request: {}", err);
                        return Ok(Response::new(Body::from(format!("Runtime error: {}", err))));
                    }
                };

                // Call user handler with deserialized object
                match handler.call(request).await {
                    Ok(resp) => match serde_json::to_vec(&resp) {
                        Ok(resp) => Ok(Response::new(Body::from(resp))),
                        Err(err) => {
                            error!("Could not serialize response");
                            return Ok(Response::new(Body::from(format!(
                                "Runtime error: {}",
                                err
                            ))));
                        }
                    },
                    Err(err) => Ok(Response::new(Body::from(format!("{}", err)))),
                }
            }
        });

        async move { Ok::<_, Infallible>(service) }
    });

    info!("Starting service");
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let server = Server::bind(&addr).serve(make_service);

    info!("Server awaiting for requests at {}", addr);
    server.await?;

    Ok(())
}