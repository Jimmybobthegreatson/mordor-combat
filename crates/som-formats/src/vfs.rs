//! Virtual file system over a game install: resolves an asset path to the bundle record
//! that the game itself would load, following `x64/default.archcfg`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

use crate::bndl::Bundle;
use crate::ltar::Archive;
use crate::normalize_path;

pub struct Vfs {
    pub root: PathBuf,
    /// In archcfg order; the game searches from the bottom up, so later wins.
    pub archives: Vec<Archive>,
    /// Opened bundles, keyed by (archive index, entry name).
    bundles: HashMap<(usize, String), Bundle>,
    /// asset path -> bundles that contain it, highest priority last.
    index: Option<HashMap<String, Vec<(usize, String)>>>,
}

impl Vfs {
    /// `root` is the install directory (the one holding the `.arch05` files).
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let cfg_path = root.join("x64").join("default.archcfg");
        let cfg = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("read {}", cfg_path.display()))?;

        // File names in the config do not always match the case on disk.
        let on_disk: HashMap<String, PathBuf> = std::fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().to_ascii_lowercase(), e.path()))
            .collect();

        let mut archives = Vec::new();
        for line in cfg.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let name = line.rsplit(['\\', '/']).next().unwrap_or(line).to_ascii_lowercase();
            if !name.ends_with(".arch05") {
                continue;
            }
            // Some configured DLC archives are simply not shipped; skip those.
            if let Some(path) = on_disk.get(&name) {
                archives.push(Archive::open(path)?);
            }
        }
        Ok(Self { root, archives, bundles: HashMap::new(), index: None })
    }

    /// Whether `dir` is a Shadow of Mordor install (it has `x64/default.archcfg`).
    pub fn is_install(dir: &Path) -> bool {
        dir.join("x64").join("default.archcfg").is_file()
    }

    /// Where the chosen install folder is remembered.
    pub fn config_file() -> Option<PathBuf> {
        let base = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("XDG_CONFIG_HOME"))
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("mordor-combat").join("install_dir.txt"))
    }

    /// Remember `dir` as the install folder for the next runs.
    pub fn save_install(dir: &Path) -> Result<()> {
        let file = Self::config_file().ok_or_else(|| anyhow!("no place to keep the setting"))?;
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(file, dir.to_string_lossy().as_bytes())?;
        Ok(())
    }

    /// Locate the install: the `SOM_DIR` environment variable, else the folder chosen on a previous run. The game asks for the folder
    /// when neither is set.
    pub fn locate() -> Result<PathBuf> {
        if let Ok(dir) = std::env::var("SOM_DIR") {
            return Ok(PathBuf::from(dir));
        }
        if let Some(saved) = Self::config_file().and_then(|f| std::fs::read_to_string(f).ok()) {
            let dir = PathBuf::from(saved.trim());
            if Self::is_install(&dir) {
                return Ok(dir);
            }
        }
        Err(anyhow!("game install not set; set SOM_DIR to your Shadow of Mordor folder (the one with the x64 folder and the .arch05 files)"))
    }

    pub fn bundle(&mut self, archive: usize, entry_name: &str) -> Result<&mut Bundle> {
        let key = (archive, entry_name.to_ascii_lowercase());
        if !self.bundles.contains_key(&key) {
            let arch = &self.archives[archive];
            let entry = arch
                .find(entry_name)
                .ok_or_else(|| anyhow!("{entry_name} not in {}", arch.path.display()))?;
            let bundle = Bundle::open(arch.reader(entry)?)
                .with_context(|| format!("{}:{entry_name}", arch.path.display()))?;
            self.bundles.insert(key.clone(), bundle);
        }
        Ok(self.bundles.get_mut(&key).unwrap())
    }

    /// Build the path index from the bundle manifests (cheap: they are small XML files).
    fn index(&mut self) -> Result<&HashMap<String, Vec<(usize, String)>>> {
        if self.index.is_none() {
            self.index = self.load_cached_index();
        }
        if self.index.is_none() {
            let mut index: HashMap<String, Vec<(usize, String)>> = HashMap::new();
            for (ai, arch) in self.archives.iter().enumerate() {
                for entry in &arch.entries {
                    let lower = entry.name.to_ascii_lowercase();
                    let Some(stem) = lower.strip_suffix(".bndlxml05") else { continue };
                    let embb = format!("{stem}.embb");
                    if arch.find(&embb).is_none() {
                        continue;
                    }
                    for (name, _) in crate::bndl::parse_manifest(&arch.read(entry)?) {
                        index.entry(normalize_path(&name)).or_default().push((ai, embb.clone()));
                    }
                }
            }
            self.store_cached_index(&index);
            self.index = Some(index);
        }
        Ok(self.index.as_ref().unwrap())
    }

    /// Reading every manifest takes ~20 s, so the path index is cached in the temp directory.
    /// The first line fingerprints the mounted archives; a mismatch discards the cache.
    fn cache_path() -> PathBuf {
        std::env::temp_dir().join("mordor-rs").join("vfs-index-v1.txt")
    }

    fn fingerprint(&self) -> String {
        self.archives
            .iter()
            .map(|a| {
                let len = std::fs::metadata(&a.path).map(|m| m.len()).unwrap_or(0);
                format!("{}:{len}", a.path.display())
            })
            .collect::<Vec<_>>()
            .join("|")
    }

    fn load_cached_index(&self) -> Option<HashMap<String, Vec<(usize, String)>>> {
        let text = std::fs::read_to_string(Self::cache_path()).ok()?;
        let mut lines = text.lines();
        if lines.next()? != self.fingerprint() {
            return None;
        }
        let mut index: HashMap<String, Vec<(usize, String)>> = HashMap::new();
        for line in lines {
            let mut parts = line.split('\t');
            let (path, archive, embb) = (parts.next()?, parts.next()?.parse().ok()?, parts.next()?);
            index.entry(path.to_string()).or_default().push((archive, embb.to_string()));
        }
        Some(index)
    }

    fn store_cached_index(&self, index: &HashMap<String, Vec<(usize, String)>>) {
        let mut text = self.fingerprint();
        for (path, bundles) in index {
            for (archive, embb) in bundles {
                text.push_str(&format!("\n{path}\t{archive}\t{embb}"));
            }
        }
        // Best effort: without the cache the index is simply rebuilt next time.
        let path = Self::cache_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }

    /// All asset paths known to the manifests.
    pub fn paths(&mut self) -> Result<Vec<String>> {
        let mut paths: Vec<String> = self.index()?.keys().cloned().collect();
        paths.sort();
        Ok(paths)
    }

    /// Read an asset by path, taking the highest-priority bundle that really holds it.
    pub fn read(&mut self, path: &str) -> Result<Vec<u8>> {
        let key = normalize_path(path);
        let candidates = self.index()?.get(&key).cloned().unwrap_or_default();
        for (archive, embb) in candidates.into_iter().rev() {
            // Some patch bundles (DLC2, NF_Patch) use a layout we do not parse yet;
            // fall through to the next-lower bundle instead of failing the lookup.
            let Ok(bundle) = self.bundle(archive, &embb) else { continue };
            // Manifests also list assets that were deduplicated out of the bundle.
            if let Some(record) = bundle.find(&key).cloned() {
                return bundle.read(&record);
            }
        }
        Err(anyhow!("asset not found: {path}"))
    }
}
