use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde_json::{Value, json};
use sonora::{platform::Account, server::Server};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Serves `total` songs. Offsets in `short` return one item fewer than asked,
/// like a server whose library shifts between pages.
struct Library {
    total: usize,
    short: Vec<usize>,
    requests: AtomicUsize,
}

async fn handler(
    State(library): State<Arc<Library>>,
    Path(endpoint): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Json<Value> {
    let body = match endpoint.as_str() {
        "search3.view" => {
            library.requests.fetch_add(1, Ordering::SeqCst);
            let offset: usize = query["songOffset"].parse().unwrap();
            let count: usize = query["songCount"].parse().unwrap();
            let count = if library.short.contains(&offset) {
                count - 1
            } else {
                count
            };
            let songs = (offset..library.total.min(offset + count))
                .map(|ix| json!({"id": ix.to_string(), "title": format!("Song {ix}")}))
                .collect::<Vec<_>>();
            json!({"status":"ok","searchResult3":{"song":songs}})
        }
        _ => json!({"status":"ok"}),
    };
    Json(json!({"subsonic-response":body}))
}

async fn sync(total: usize, short: Vec<usize>, ahead: usize) -> (Vec<usize>, usize) {
    let library = Arc::new(Library {
        total,
        short,
        requests: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/rest/{endpoint}", get(handler))
        .with_state(library.clone());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = Server::new(
        Account {
            url: format!("http://{address}"),
            username: "test".into(),
        },
        "secret".into(),
    )
    .unwrap();
    let mut seen = Vec::new();
    server
        .each_page("song", ahead, |page| {
            seen.extend(
                page.iter()
                    .map(|song| song["id"].as_str().unwrap().parse::<usize>().unwrap()),
            );
            async { Ok(()) }
        })
        .await
        .unwrap();
    task.abort();
    (seen, library.requests.load(Ordering::SeqCst))
}

#[tokio::test]
async fn pages_arrive_in_order_with_requests_in_flight() {
    let (seen, _) = sync(1234, vec![], 4).await;
    assert_eq!(seen, (0..1234).collect::<Vec<_>>());
    // An exact multiple of the page size ends on an empty page.
    let (seen, _) = sync(1000, vec![], 3).await;
    assert_eq!(seen, (0..1000).collect::<Vec<_>>());
    let (seen, requests) = sync(0, vec![], 4).await;
    assert!(seen.is_empty());
    assert!(requests >= 1);
}

#[tokio::test]
async fn a_short_page_resumes_from_where_it_ended() {
    // The page at 500 holds 499 songs, so the next page must start at 999,
    // exactly as fetching one page at a time would.
    let (seen, _) = sync(1600, vec![500], 4).await;
    assert_eq!(seen, (0..1600).collect::<Vec<_>>());
}
