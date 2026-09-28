//! The saved gateways: what the library lists, in the order its rows are dragged into,
//! which one its form was last showing and where its window was left, in
//! `profiles.json` in the viewer's data directory.
//!
//! One viewer runs at a time (`os::claim_instance`), so this is the file's only writer
//! and what it holds is the file as it is. A change is made to a copy, written beside
//! the file and moved over it, and only then kept: a change that could not be written
//! is not kept either, and a write cut short leaves the last whole list.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// A saved gateway: a name, and the URL its page is at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub url: String,
}

/// Where a window was left, in the screen's own pixels: its outer position and size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    profiles: Vec<Profile>,
    selected: Option<String>,
    library: Option<Placement>,
}

pub struct Store {
    path: PathBuf,
    saved: Saved,
}

/// `text` as the URL of a gateway's page: an http or https address, and a bare host
/// is an https one.
pub fn gateway_url(text: &str) -> Result<String> {
    let text = text.trim();
    anyhow::ensure!(!text.is_empty(), "A gateway URL is needed.");
    let url = match url::Url::parse(text) {
        Err(url::ParseError::RelativeUrlWithoutBase) => url::Url::parse(&format!("https://{text}")),
        parsed => parsed,
    }
    .with_context(|| format!("“{text}” is not a URL."))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.has_host(),
        "A gateway URL is an http:// or https:// address."
    );
    Ok(url.into())
}

impl Store {
    /// The list in `dir`. None saved yet is an empty list, and so is a file that is not
    /// one — there is nothing in it to keep — or one that cannot be read, which saving
    /// to then says why.
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("profiles.json");
        let saved = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                eprintln!("remotex-viewer: {} is not a list of gateways, so the list starts empty: {e}", path.display());
                Saved::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Saved::default(),
            Err(e) => {
                eprintln!("remotex-viewer: {} cannot be read, so the list starts empty: {e}", path.display());
                Saved::default()
            }
        };
        Self { path, saved }
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.saved.profiles
    }

    pub fn find(&self, id: &str) -> Option<&Profile> {
        self.saved.profiles.iter().find(|p| p.id == id)
    }

    /// The one the form was showing when the viewer was last used.
    pub fn selected(&self) -> Option<&str> {
        self.saved.selected.as_deref()
    }

    /// Where the library was when it was last put away or closed.
    pub fn library(&self) -> Option<Placement> {
        self.saved.library
    }

    /// The first profile at `url`: what a launch with a URL selects in the library.
    pub fn matching(&self, url: &str) -> Option<&Profile> {
        self.saved.profiles.iter().find(|p| p.url == url)
    }

    /// Save `name` and `url` into the profile `id`, or into a new one after the rest
    /// when `id` is none, which the form then shows. Answers the profile as saved.
    pub fn put(&mut self, id: Option<&str>, name: &str, url: &str) -> Result<Profile> {
        let profile = Profile {
            id: id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned),
            name: name.trim().to_owned(),
            url: gateway_url(url)?,
        };
        self.change(|saved| {
            match saved.profiles.iter_mut().find(|p| p.id == profile.id) {
                Some(stored) => *stored = profile.clone(),
                None => {
                    saved.profiles.push(profile.clone());
                    saved.selected = Some(profile.id.clone());
                }
            }
        })?;
        Ok(profile)
    }

    pub fn remove(&mut self, id: &str) -> Result<()> {
        self.change(|saved| {
            saved.profiles.retain(|p| p.id != id);
            if saved.selected.as_deref() == Some(id) {
                saved.selected = None;
            }
        })
    }

    /// Put the profiles in the order of `order`'s ids; one it does not name keeps its
    /// place after the rest.
    pub fn reorder(&mut self, order: &[String]) -> Result<()> {
        self.change(|saved| {
            saved.profiles.sort_by_key(|p| order.iter().position(|id| *id == p.id).unwrap_or(usize::MAX));
        })
    }

    /// Not keeping which one was showing is no reason to stop anything, so a write
    /// that fails is only reported.
    pub fn set_selected(&mut self, id: Option<String>) {
        if self.saved.selected != id {
            self.keep(|saved| saved.selected = id);
        }
    }

    /// Nor is not keeping where the library was.
    pub fn set_library(&mut self, placement: Placement) {
        if self.saved.library != Some(placement) {
            self.keep(|saved| saved.library = Some(placement));
        }
    }

    fn keep(&mut self, change: impl FnOnce(&mut Saved)) {
        if let Err(e) = self.change(change) {
            eprintln!("remotex-viewer: {e:#}");
        }
    }

    fn change(&mut self, change: impl FnOnce(&mut Saved)) -> Result<()> {
        let mut next = self.saved.clone();
        change(&mut next);
        self.write(&next).context("The gateways could not be saved")?;
        self.saved = next;
        Ok(())
    }

    fn write(&self, saved: &Saved) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        let partial = self.path.with_extension("json.partial");
        std::fs::write(&partial, serde_json::to_vec_pretty(saved)?)
            .with_context(|| format!("write {}", partial.display()))?;
        std::fs::rename(&partial, &self.path).with_context(|| format!("replace {}", self.path.display()))
    }
}
