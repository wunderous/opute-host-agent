//! Private connected-candidate catalog ownership from provider_candidate.go.
//! Lookup binds an immutable descriptor; execution still requires ordinary admission.
use super::{adapter::Adapter, definition::InstallManifest};
use crate::catalog::{provider_descriptors, with_provider_generation, Descriptor, Snapshot};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct State {
    manifests: BTreeMap<String, InstallManifest>,
    adapters: BTreeMap<String, Option<Arc<Adapter>>>,
}
#[derive(Default)]
pub struct Candidates {
    state: Mutex<State>,
}
impl Candidates {
    pub fn manifest_entries(&self) -> BTreeMap<String, InstallManifest> {
        self.state.lock().unwrap().manifests.clone()
    }
    pub fn remove_manifest(&self, generation: &str) {
        self.state.lock().unwrap().manifests.remove(generation);
    }
    pub fn manifest(&self, generation: &str) -> Option<InstallManifest> {
        self.state
            .lock()
            .unwrap()
            .manifests
            .get(generation)
            .cloned()
    }
    pub fn manifests(&self) -> Vec<InstallManifest> {
        self.state
            .lock()
            .unwrap()
            .manifests
            .values()
            .cloned()
            .collect()
    }
    pub fn set_manifest(&self, generation: &str, manifest: InstallManifest) {
        self.state
            .lock()
            .unwrap()
            .manifests
            .insert(generation.into(), manifest);
    }
    pub fn set_adapter(&self, generation: &str, adapter: Option<Arc<Adapter>>) {
        self.state
            .lock()
            .unwrap()
            .adapters
            .insert(generation.into(), adapter);
    }
    pub fn remove(&self, generation: &str) {
        let mut state = self.state.lock().unwrap();
        state.manifests.remove(generation);
        state.adapters.remove(generation);
    }
    pub fn snapshot(&self, active: &Snapshot, generation: &str) -> Option<Snapshot> {
        if generation.trim().is_empty() {
            return None;
        }
        let manifest = {
            let state = self.state.lock().unwrap();
            if !state.adapters.contains_key(generation) {
                return None;
            }
            state.manifests.get(generation)?.clone()
        };
        Some(with_provider_generation(active, &manifest, generation))
    }
    pub fn descriptor(&self, generation: &str, name: &str) -> Option<Descriptor> {
        let manifest = self
            .state
            .lock()
            .unwrap()
            .manifests
            .get(generation)?
            .clone();
        provider_descriptors(&manifest, generation)
            .into_iter()
            .find(|descriptor| descriptor.name == name)
    }
    pub fn adapter(&self, generation: &str) -> Option<Arc<Adapter>> {
        self.state
            .lock()
            .unwrap()
            .adapters
            .get(generation)
            .cloned()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_candidate_revokes_lookup_without_mutating_retained_snapshot() {
        let candidates = Candidates::default();
        let active = crate::catalog::for_mode(true);
        let generation = " opaque/@generation ";
        let manifest = InstallManifest::default();
        candidates.set_manifest(generation, manifest);
        assert!(candidates.snapshot(active, generation).is_none());
        candidates.set_adapter(generation, None);
        let accepted = candidates.snapshot(active, generation).unwrap();
        let revision = accepted.revision.clone();
        candidates.remove(generation);
        assert!(candidates.snapshot(active, generation).is_none());
        assert!(candidates.adapter(generation).is_none());
        assert_eq!(accepted.revision, revision);
        assert_eq!(active.revision, crate::catalog::for_mode(true).revision);
    }
}
