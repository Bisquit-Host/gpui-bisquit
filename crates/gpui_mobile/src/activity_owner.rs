/// Android may create the next Activity before destroying the previous one.
/// A late callback must never replace or release the newer Activity's surface.
#[derive(Debug, Default)]
pub(crate) struct ActivityOwner(u64);

impl ActivityOwner {
    pub(crate) fn attach(&mut self, owner: u64) -> bool {
        if owner < self.0 {
            return false;
        }
        self.0 = owner;
        true
    }

    pub(crate) fn owns(&self, owner: u64) -> bool {
        owner == self.0
    }
}

#[cfg(test)]
mod tests {
    use super::ActivityOwner;

    #[test]
    fn late_destroy_cannot_detach_the_replacement_activity() {
        let mut owner = ActivityOwner::default();
        assert!(owner.attach(1));
        assert!(owner.attach(2));
        assert!(!owner.owns(1));
        assert!(owner.owns(2));
    }

    #[test]
    fn late_surface_change_cannot_reclaim_the_window() {
        let mut owner = ActivityOwner::default();
        assert!(owner.attach(1));
        assert!(owner.attach(2));
        assert!(!owner.attach(1));
        assert!(owner.attach(2));
        assert!(owner.owns(2));
    }
}
