use super::{ARTIST, Fixture, SECOND_ARTIST};
use sonora::{model::LibraryQuery, service::Event};

impl Fixture {
    pub(crate) fn hold_artists(&mut self) {
        self.hold_artists = true;
    }
    pub(crate) fn deliver_artist(&mut self, name: &str) {
        let at = self
            .held_views
            .iter()
            .position(|(key, event)| key == name && matches!(event, Event::Artist(..)))
            .unwrap();
        let (_, event) = self.held_views.remove(at).unwrap();
        self.backend.emit(event);
    }

    pub(crate) fn hold_queries(&mut self) {
        self.hold_queries = true;
    }

    pub(crate) fn deliver_query(&mut self, text: &str) {
        let at = self
            .held_views
            .iter()
            .position(|(query, _)| query == text)
            .expect("held search result");
        let (_, event) = self.held_views.remove(at).unwrap();
        let Event::View(request, _) = &event else {
            unreachable!("held query view")
        };
        self.trace
            .push(format!("delivered held View({request}) for {text:?}"));
        self.backend.emit(event);
    }

    pub(crate) fn last_query(&self) -> &LibraryQuery {
        self.query_requests.last().expect("query sent by UI")
    }

    pub(super) fn respond_query(&mut self, request: u64, query: LibraryQuery) {
        self.trace.push(format!("Query({request}) -> View"));
        let event = Event::View(request, self.catalog.view(&query).unwrap());
        self.query_requests.push(query.clone());
        if self.hold_queries {
            self.held_views.push_back((query.text, event));
        } else {
            self.backend.emit(event);
        }
    }

    pub(super) fn respond_artist(&mut self, request: u64, name: String, id: String) {
        self.trace.push(format!("Artist({request}) -> Artist"));
        assert!(
            [(ARTIST, "artist-1"), (SECOND_ARTIST, "artist-2")]
                .contains(&(name.as_str(), id.as_str()))
        );
        let event = Event::Artist(request, self.catalog.artist_page(&name, &id).unwrap());
        if self.hold_artists {
            self.held_views.push_back((name, event));
        } else {
            self.backend.emit(event);
        }
    }

    pub(super) fn respond_group_tracks(&mut self, request: u64, query: LibraryQuery, id: String) {
        self.trace
            .push(format!("GroupTracks({request}, {id}) -> GroupTracks"));
        self.backend.emit(Event::GroupTracks(
            request,
            id.clone(),
            self.catalog.album_tracks_matching(&query, &id).unwrap(),
        ));
    }
}
