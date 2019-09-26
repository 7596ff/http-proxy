mod error;

use dawn_http::{
    client::Client,
    request::Request as DawnRequest,
    routing::Path,
};
use error::{
    ChunkingRequest,
    ChunkingResponse,
    InvalidPath,
    MakingResponseBody,
    RequestError,
    RequestIssue,
};
use futures::TryStreamExt;
use http::request::Parts;
use hyper::{
    body::Body,
    server::{
        conn::AddrStream,
        Server,
    },
    service,
    Request,
    Response,
};
use log::{debug, error, info};
use snafu::ResultExt;
use std::{
    convert::TryFrom,
    env,
    error::Error,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    pretty_env_logger::try_init_timed()?;

    let host_raw = env::var("HOST").unwrap_or("0.0.0.0".into());
    let host = IpAddr::from_str(&host_raw)?;
    let port = env::var("PORT").unwrap_or("80".into()).parse()?;

    let client = Client::new(env::var("DISCORD_TOKEN")?);

    let address = SocketAddr::from((host, port));

    // The closure inside `make_service_fn` is run for each connection,
    // creating a 'service' to handle requests for that specific connection.
    let service = service::make_service_fn(move |addr: &AddrStream| {
        debug!("Connection from: {:?}", addr);
        let client = client.clone();

        async move {
            Ok::<_, RequestError>(service::service_fn(move |incoming: Request<Body>| {
                handle_request(client.clone(), incoming)
            }))
        }
    });

    let server = Server::bind(&address)
        .serve(service);

    info!("Listening on http://{}", address);

    if let Err(why) = server.await {
        error!("Fatal server error: {}", why);
    }

    Ok(())
}

async fn handle_request(client: Client, request: Request<Body>) -> Result<Response<Body>, RequestError> {
    debug!("Incoming request: {:?}", request);

    let (parts, body) = request.into_parts();
    let Parts { method, uri, headers, .. } = parts;

    let trimmed_path = if uri.path().starts_with("/api/v6") {
        uri.path().replace("/api/v6", "")
    } else {
        uri.path().to_owned()
    };
    let path = Path::try_from((
        method.clone(),
        trimmed_path.as_ref(),
    )).context(InvalidPath)?;

    let bytes = (*body.try_concat().await.context(ChunkingRequest)?).to_owned();

    let path_and_query = match uri.path_and_query() {
        Some(v) => v.as_str().replace("/api/v6/", "").into(),
        None => {
            debug!("No path in URI: {:?}", uri);

            return Err(RequestError::NoPath {
                uri,
            });
        },
    };
    let raw_request = DawnRequest {
        body: Some(bytes),
        headers: Some(headers),
        method,
        path,
        path_str: path_and_query,
    };

    let resp = client.raw(raw_request).await.context(RequestIssue)?;

    let status = resp.status();
    let resp_headers = resp.headers().clone();

    let bytes = resp.bytes().await.context(ChunkingResponse)?;

    let mut builder = Response::builder();
    builder.status(status);

    if let Some(headers) = builder.headers_mut() {
        headers.extend(resp_headers);
    }

    let resp = builder.body(Body::from(bytes)).context(MakingResponseBody)?;

    debug!("Response: {:?}", resp);

    Ok(resp)
}
