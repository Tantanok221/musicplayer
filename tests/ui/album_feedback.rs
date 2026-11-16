use crate::common::ui::{Fixture, MUTATION_ERROR, start};
use gpui_kit::{AppContext, TestAppContext, test::TestWindowExt};
use sonora::service::Mutation;

fn assert_favorite(
    window: gpui_kit::AnyWindowHandle,
    favorite: bool,
    pending: bool,
    cx: &mut TestAppContext,
) {
    cx.update_window(window, |_, window, _| {
        let button = window.find("album-favorite-album-1");
        assert!(button.visible());
        assert_eq!(
            button.label(),
            Some(if favorite {
                "Unfavorite album Fixture album"
            } else {
                "Favorite album Fixture album"
            })
        );
        assert_eq!(button.disabled() == Some(true), pending);
        assert_eq!(
            window.find("album-rating-album-1-4").disabled() == Some(true),
            pending
        );
    })
    .unwrap();
}

fn assert_song_and_other_album_unchanged(fixture: &Fixture) {
    let tracks = fixture.catalog.by_ids(&["track-1".into()]).unwrap();
    assert_eq!(tracks[0].user_rating, Some(5));
    assert_eq!(tracks[0].starred.as_deref(), Some("2026-01-01T00:00:00Z"));
    let other = fixture.catalog.album("album-2").unwrap().unwrap();
    assert_eq!(other.user_rating, Some(4));
    assert_eq!(other.starred.as_deref(), Some("2026-01-01T00:00:00Z"));
}

#[gpui_kit::test]
fn album_favorite_is_optimistic_blocks_duplicates_and_confirms(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.open_album(window, cx);
    assert_favorite(window, false, false, cx);
    fixture.click(window, "album-favorite-album-1", cx);
    fixture.diagnose("favorite pending", window, &app, cx);
    assert_favorite(window, true, true, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    fixture.click(window, "album-favorite-album-1", cx);
    assert_eq!(
        fixture.mutations(),
        &[Mutation::AlbumFavorite("album-1".into(), true)]
    );

    fixture.confirm_mutation();
    fixture.settle(window, cx);
    assert_favorite(window, true, false, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_some()
    );
    // A real second click proves the confirmed control is enabled again.
    fixture.click(window, "album-favorite-album-1", cx);
    assert_favorite(window, false, true, cx);
    assert_eq!(
        fixture.mutations(),
        &[
            Mutation::AlbumFavorite("album-1".into(), true),
            Mutation::AlbumFavorite("album-1".into(), false)
        ]
    );
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    assert_favorite(window, false, false, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    assert_song_and_other_album_unchanged(&fixture);
}

#[gpui_kit::test]
fn failed_album_favorite_rolls_back_and_retry_sends_the_same_intent(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.open_album(window, cx);
    fixture.click(window, "album-favorite-album-1", cx);
    assert_favorite(window, true, true, cx);
    fixture.fail_mutation();
    fixture.settle(window, cx);
    fixture.diagnose("favorite rolled back", window, &app, cx);
    assert_favorite(window, false, false, cx);
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_none()
    );
    cx.update_window(window, |_, window, _| {
        let error = window.find("error-banner");
        assert_eq!(error.role(), Some(gpui_kit::Role::Alert));
        assert_eq!(error.label(), Some(MUTATION_ERROR));
        assert!(error.visible());
        assert!(window.find("retry-error").visible());
    })
    .unwrap();

    fixture.click(window, "retry-error", cx);
    assert_favorite(window, true, true, cx);
    assert_eq!(
        fixture.mutations(),
        &[
            Mutation::AlbumFavorite("album-1".into(), true),
            Mutation::AlbumFavorite("album-1".into(), true)
        ]
    );
    fixture.click(window, "album-favorite-album-1", cx);
    assert_eq!(fixture.mutations().len(), 2);
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    fixture.diagnose("favorite retry confirmed", window, &app, cx);
    assert_favorite(window, true, false, cx);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find("error-banner").is_none());
        assert!(window.try_find("retry-error").is_none());
    })
    .unwrap();
    assert!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .starred
            .is_some()
    );
    assert_song_and_other_album_unchanged(&fixture);
}

#[gpui_kit::test]
fn album_rating_sets_and_clears_without_changing_song_rating(cx: &mut TestAppContext) {
    let (_dir, mut fixture, window, app) = start(cx);
    fixture.open_album(window, cx);
    cx.update_window(window, |_, window, _| {
        assert_eq!(
            window.find("album-rating-album-1-2").label(),
            Some("Clear album Fixture album rating")
        );
    })
    .unwrap();
    fixture.click(window, "album-rating-album-1-4", cx);
    fixture.diagnose("album rating pending", window, &app, cx);
    assert_eq!(
        fixture.mutations(),
        &[Mutation::AlbumRating("album-1".into(), 4)]
    );
    assert_eq!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(2)
    );
    cx.update_window(window, |_, window, _| {
        let rating = window.find("album-rating-album-1-4");
        assert_eq!(rating.label(), Some("Clear album Fixture album rating"));
        assert_eq!(rating.disabled(), Some(true));
    })
    .unwrap();
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    assert_eq!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(4)
    );
    fixture.click(window, "album-rating-album-1-4", cx);
    assert_eq!(
        fixture.mutations(),
        &[
            Mutation::AlbumRating("album-1".into(), 4),
            Mutation::AlbumRating("album-1".into(), 0)
        ]
    );
    fixture.confirm_mutation();
    fixture.settle(window, cx);
    fixture.diagnose("album rating cleared", window, &app, cx);
    assert_eq!(
        fixture
            .catalog
            .album("album-1")
            .unwrap()
            .unwrap()
            .user_rating,
        Some(0)
    );
    cx.update_window(window, |_, window, _| {
        let rating = window.find("album-rating-album-1-4");
        assert_eq!(rating.label(), Some("Rate album Fixture album 4 stars"));
        assert_ne!(rating.disabled(), Some(true));
    })
    .unwrap();
    assert_song_and_other_album_unchanged(&fixture);
}
