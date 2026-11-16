use crate::common::ui::start;
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::service::{Command, Event, Mutation};

#[gpui_kit::test]
fn song_favorite_and_rating_are_optimistic_roll_back_and_retry(cx: &mut TestAppContext) {
    let (_dir, mut f, window, _) = start(cx);
    f.open_album(window, cx);
    let original = f.catalog.by_ids(&["track-1".into()]).unwrap()[0].clone();
    for (control, mutation, optimistic) in [
        (
            "love-group-album-1-0",
            Mutation::Favorite("track-1".into(), false),
            "Fixture song: not favorite, rating 5 of 5",
        ),
        (
            "rating-group-album-1-0-3",
            Mutation::Rating("track-1".into(), 3),
            "Fixture song: favorite, rating 3 of 5",
        ),
    ] {
        let expected = mutation.clone();
        f.expect("song feedback", move |command, _| {
            assert!(matches!(command, Command::Mutate(actual) if actual == expected));
            vec![]
        });
        f.click(window, control, cx);
        cx.update_window(window, |_, window, _| {
            assert_eq!(
                window.find("song-feedback-group-album-1-0").label(),
                Some(optimistic)
            );
        })
        .unwrap();
        // The database remains authoritative until confirmation.
        assert_eq!(
            f.catalog.by_ids(&["track-1".into()]).unwrap()[0].user_rating,
            Some(5)
        );
        f.backend.emit(Event::TrackUpdated(original.clone()));
        f.backend.emit(Event::MutationFailed(
            mutation.clone(),
            "Song update rejected".into(),
        ));
        f.settle(window, cx);
        cx.update_window(window, |_, window, _| {
            assert_eq!(
                window.find("song-feedback-group-album-1-0").label(),
                Some("Fixture song: favorite, rating 5 of 5")
            );
            assert!(window.find("retry-error").visible());
        })
        .unwrap();
        let expected = mutation.clone();
        f.expect("same song intent on Retry", move |command, _| {
            assert!(matches!(command, Command::Mutate(actual) if actual == expected));
            vec![]
        });
        f.click(window, "retry-error", cx);
        let mut confirmed = (*original).clone();
        match &mutation {
            Mutation::Favorite(_, value) => confirmed.starred = value.then(|| "fixture".into()),
            Mutation::Rating(_, value) => confirmed.user_rating = Some(*value),
            _ => unreachable!(),
        }
        f.backend
            .emit(Event::TrackUpdated(std::sync::Arc::new(confirmed)));
        f.backend.emit(Event::MutationDone(mutation));
        f.settle(window, cx);
        cx.update_window(window, |_, window, _| {
            assert_eq!(
                window.find("song-feedback-group-album-1-0").label(),
                Some(optimistic)
            );
            assert!(window.try_find("retry-error").is_none());
        })
        .unwrap();
        f.backend.emit(Event::TrackUpdated(original.clone()));
        f.settle(window, cx);
    }
    f.assert_script_finished();
}
