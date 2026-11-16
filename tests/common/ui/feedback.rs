use super::{Fixture, MUTATION_ERROR};
use sonora::service::{Event, Mutation};

impl Fixture {
    pub(crate) fn mutations(&self) -> &[Mutation] {
        &self.mutations
    }

    pub(super) fn hold_mutation(&mut self, mutation: Mutation) {
        self.trace.push(format!("held {mutation:?}"));
        self.mutations.push(mutation.clone());
        self.pending_mutations.push_back(mutation);
    }

    pub(crate) fn confirm_mutation(&mut self) {
        let mutation = self
            .pending_mutations
            .pop_front()
            .expect("held mutation to confirm");
        let album = match &mutation {
            Mutation::AlbumFavorite(id, favorite) => {
                self.catalog
                    .update_album_feedback(id, Some(*favorite), None)
            }
            Mutation::AlbumRating(id, rating) => {
                self.catalog.update_album_feedback(id, None, Some(*rating))
            }
            _ => unreachable!("only album feedback is held"),
        }
        .unwrap()
        .expect("fixture album");
        self.trace.push("AlbumUpdated -> MutationDone".into());
        self.backend.emit(Event::AlbumUpdated(album));
        self.backend.emit(Event::MutationDone(mutation));
    }

    pub(crate) fn fail_mutation(&mut self) {
        let mutation = self
            .pending_mutations
            .pop_front()
            .expect("held mutation to fail");
        let album = self
            .catalog
            .album(mutation.album_feedback_id().unwrap())
            .unwrap()
            .unwrap();
        self.trace
            .push("original AlbumUpdated -> MutationFailed".into());
        self.backend.emit(Event::AlbumUpdated(album));
        self.backend
            .emit(Event::MutationFailed(mutation, MUTATION_ERROR.into()));
    }
}
