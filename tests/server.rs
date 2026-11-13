use axum::{
    Json, Router,
    extract::{Path, Query},
    routing::get,
};
use serde_json::{Value, json};
use sonora::{platform::Account, server::Server};
use std::collections::HashMap;

async fn handler(
    Path(endpoint): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    uri: axum::http::Uri,
) -> Json<Value> {
    assert_eq!(query.get("u").unwrap(), "test");
    assert!(!query.contains_key("p"));
    let expected = format!(
        "{:x}",
        md5::compute(format!("secret{}", query.get("s").unwrap()))
    );
    assert_eq!(query.get("t").unwrap(), &expected);
    let body = match endpoint.as_str() {
        "ping.view" => json!({"status":"ok","type":"navidrome","serverVersion":"test"}),
        "getOpenSubsonicExtensions.view" => {
            json!({"status":"ok","openSubsonicExtensions":[{"name":"songLyrics","versions":[1]}]})
        }
        "search3.view" => {
            assert_eq!(query["query"], "");
            assert_eq!(query["songCount"], "500");
            assert_eq!(query["songOffset"], "500");
            json!({"status":"ok","searchResult3":{"song":[{"id":"1","title":"夜の散歩","albumId":"a"}]}})
        }
        "getScanStatus.view" => json!({"status":"ok","scanStatus":{"scanning":false}}),
        "star.view" => json!({"status":"ok"}),
        "updatePlaylist.view" => {
            assert_eq!(query["playlistId"], "p1");
            // Every song is sent, in order, as its own parameter.
            let raw = uri.query().unwrap();
            assert!(raw.contains("songIdToAdd=1&songIdToAdd=2"), "{raw}");
            json!({"status":"ok"})
        }
        "getLyricsBySongId.view" => match query["id"].as_str() {
            "synced" => json!({"status":"ok","lyricsList":{"structuredLyrics":[
                {"synced":false,"line":[{"value":"Plain version"}]},
                {"synced":true,"offset":-250,"line":[{"start":1000,"value":"夜の散歩"},{"start":2500,"value":"Second line"}]}
            ]}}),
            "legacy" => json!({"status":"failed","error":{"code":70,"message":"Not supported"}}),
            "empty" => json!({"status":"ok","lyricsList":{"structuredLyrics":[]}}),
            _ => {
                json!({"status":"ok","lyricsList":{"structuredLyrics":[{"line":[{"value":"First line"},{"value":"Second line"}]}]}})
            }
        },
        "getLyrics.view" => json!({"status":"ok","lyrics":{"value":"Legacy line\nNext line"}}),
        _ => json!({"status":"failed","error":{"code":40,"message":"Authentication failed"}}),
    };
    Json(json!({"subsonic-response":body}))
}
#[tokio::test]
async fn authentication_base_path_capabilities_and_pagination() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route("/music/rest/{endpoint}", get(handler)),
        )
        .await
        .unwrap();
    });
    let mut server = Server::new(
        Account {
            url: format!("http://{address}/music"),
            username: "test".into(),
        },
        "secret".into(),
    )
    .unwrap();
    assert!(server.connect().await.unwrap().contains("navidrome"));
    let mut resumed =
        Server::with_credentials(server.account.clone(), server.credentials().clone()).unwrap();
    assert!(resumed.connect().await.unwrap().contains("navidrome"));
    assert!(server.extensions.contains("songLyrics"));
    assert_eq!(
        server.page("song", 500).await.unwrap()[0]["title"],
        "夜の散歩"
    );
    assert!(!server.scan_status().await.unwrap());
    server.star("1", true).await.unwrap();
    server
        .add_to_playlist("p1", &["1".into(), "2".into()])
        .await
        .unwrap();
    assert_eq!(
        server
            .lyrics(&sonora::model::Track {
                id: "1".into(),
                ..Default::default()
            })
            .await
            .unwrap()
            .line
            .iter()
            .map(|line| line.value.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        "First line\nSecond line"
    );
    let lyrics = server
        .lyrics(&sonora::model::Track {
            id: "synced".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(lyrics.is_timed());
    assert_eq!(lyrics.offset, -250);
    assert_eq!(lyrics.line[0].value, "夜の散歩");
    assert_eq!(lyrics.line[0].start, Some(1000));
    assert_eq!(lyrics.active_line(1.249), None);
    assert_eq!(lyrics.active_line(1.25), Some(0));
    for id in ["legacy", "empty"] {
        let lyrics = server
            .lyrics(&sonora::model::Track {
                id: id.into(),
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(!lyrics.is_timed());
        assert_eq!(lyrics.line[0].value, "Legacy line");
    }
    let error = server.rating("1", 5).await.unwrap_err().to_string();
    assert!(error.contains("Authentication failed"));
    assert!(!error.contains("secret"));
    assert!(!error.contains("?u="));
    task.abort();
}
#[test]
fn invalid_accounts_are_rejected_and_namespace_is_account_specific() {
    assert!(
        Server::new(
            Account {
                url: "file:///tmp/music".into(),
                username: "a".into()
            },
            "x".into()
        )
        .is_err()
    );
    assert!(
        Server::new(
            Account {
                url: "https://a:b@example.com".into(),
                username: "a".into()
            },
            "x".into()
        )
        .is_err()
    );
    let a = Account {
        url: "https://example.com/".into(),
        username: "a".into(),
    };
    let b = Account {
        username: "b".into(),
        ..a.clone()
    };
    assert_ne!(a.namespace(), b.namespace());
}
