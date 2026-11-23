use super::*;

#[tokio::test]
async fn only_a_successful_target_ack_moves_playback_authority() {
    let dir = tempfile::tempdir().unwrap();
    let (navidrome, _) = fake_navidrome().await;
    let relay = start(&navidrome, &dir).await;
    let (mut source, _) = login(&relay, "alice", "source").await;
    let (mut target, _) = login(&relay, "alice", "target").await;
    assert!(matches!(
        next(&mut source).await,
        ServerMessage::DevicesChanged { .. }
    ));
    let state = PlaybackState {
        queue: vec!["song".into(), "song".into()],
        index: Some(1),
        position_ms: 123,
        playing: true,
        seq: 4,
        ..Default::default()
    };
    send(&mut source, ClientMessage::StateUpdate { state }).await;
    assert!(matches!(
        next(&mut target).await,
        ServerMessage::State { .. }
    ));

    for acknowledge in [false, true] {
        send(
            &mut source,
            ClientMessage::Transfer {
                target_device_id: "target".into(),
                start_playing: true,
            },
        )
        .await;
        let transfer_id = match tokio::select! {
            message = next(&mut target) => message,
            message = next(&mut source) => panic!("source got {message:?} before target prepared the queue"),
        } {
            ServerMessage::TransferRequest {
                transfer_id,
                state,
                start_playing: true,
            } => {
                assert_eq!(state.index, Some(1));
                assert_eq!(state.queue, ["song", "song"]);
                assert!(state.position_ms >= 123);
                transfer_id
            }
            other => panic!("expected transfer request, got {other:?}"),
        };
        send(&mut source, ClientMessage::Ping).await;
        assert!(matches!(next(&mut source).await, ServerMessage::Pong));
        if !acknowledge {
            let message = tokio::time::timeout(Duration::from_secs(10), source.next())
                .await
                .expect("transfer must time out")
                .unwrap()
                .unwrap();
            let result: ServerMessage = serde_json::from_str(message.to_text().unwrap()).unwrap();
            assert!(
                matches!(result, ServerMessage::TransferResult {transfer_id: id, ok: false, ..} if id == transfer_id)
            );
            assert!(
                matches!(next(&mut target).await, ServerMessage::TransferResult {transfer_id: id, ok: false, ..} if id == transfer_id)
            );
            continue;
        }
        send(
            &mut target,
            ClientMessage::TransferAck {
                transfer_id: transfer_id.clone(),
                ok: true,
                error: None,
            },
        )
        .await;
        assert!(matches!(
            next(&mut source).await,
            ServerMessage::Command {
                cmd: PlaybackCommand::Pause,
                ..
            }
        ));
        assert!(
            matches!(next(&mut source).await, ServerMessage::ActiveDeviceChanged {device_id: Some(id)} if id == "target")
        );
        assert!(
            matches!(next(&mut source).await, ServerMessage::State {from_device_id, state} if from_device_id == "target" && state.seq > 4 && state.playing)
        );
        assert!(
            matches!(next(&mut source).await, ServerMessage::TransferResult {transfer_id: id, ok: true, ..} if id == transfer_id)
        );
    }
}
