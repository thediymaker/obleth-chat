//! Reading the answers of the services Chat calls: the gateway, decision endpoints
//! and clusters.

use futures_util::StreamExt;

/// Why an answer's body could not be read.
#[derive(Debug, PartialEq)]
pub(crate) enum Read {
    /// The connection failed before the whole body arrived.
    Closed,
    /// The body is larger than allowed, by its Content-Length or as it arrived.
    TooLarge,
}

/// Reads a body, giving up on one larger than `max` instead of holding it all in
/// memory. A Content-Length over `max` is refused before anything is read.
pub(crate) async fn read_capped(response: reqwest::Response, max: usize) -> Result<Vec<u8>, Read> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(Read::TooLarge);
    }
    let mut body = vec![];
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Read::Closed)?;
        if body.len() + chunk.len() > max {
            return Err(Read::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::{read_capped, Read};
    use axum::{body::Body, routing::get, Router};

    /// Serves `size` bytes at /fixed with a Content-Length and at /streamed without.
    async fn server(size: usize) -> String {
        let app = Router::new()
            .route("/fixed", get(move || async move { vec![b'x'; size] }))
            .route(
                "/streamed",
                get(move || async move {
                    let parts = vec![b'x'; size]
                        .chunks(7)
                        .map(|c| Ok::<_, std::io::Error>(c.to_vec()))
                        .collect::<Vec<_>>();
                    Body::from_stream(futures_util::stream::iter(parts))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn reads_bodies_up_to_the_limit_with_or_without_a_length() {
        let base = server(100).await;
        let client = reqwest::Client::new();
        for path in ["fixed", "streamed"] {
            let get = || client.get(format!("{base}/{path}")).send();
            let response = get().await.unwrap();
            if path == "fixed" {
                assert_eq!(response.content_length(), Some(100));
            } else {
                assert_eq!(response.content_length(), None);
            }
            assert_eq!(
                read_capped(response, 100).await.unwrap().len(),
                100,
                "{path} at"
            );
            assert_eq!(
                read_capped(get().await.unwrap(), 101).await.unwrap().len(),
                100,
                "{path} below"
            );
            assert_eq!(
                read_capped(get().await.unwrap(), 99).await,
                Err(Read::TooLarge),
                "{path} above"
            );
        }
    }
}
