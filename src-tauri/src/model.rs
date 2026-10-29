use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Windows,
    #[default]
    Unknown,
}

impl Os {
    pub fn host() -> Self {
        if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Unknown
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Source {
    pub path: String,
    #[serde(default)]
    pub os: Os,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PathMapping {
    pub original: String,
    pub mounted: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ScanOptions {
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub mappings: Vec<PathMapping>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Installation {
    pub library: String,
    pub path: String,
    pub os: Os,
    // installed, incomplete, files_missing, unavailable
    pub state: String,
    pub last_seen: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    pub appid: u32,
    pub title: String,
    pub owned: bool,
    pub installations: Vec<Installation>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Volume {
    pub id: String,
    pub device: String,
    pub filesystem: String,
    pub mountpoints: Vec<String>,
    pub os: Os,
    pub note: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub host_os: Os,
    pub games: Vec<Game>,
    pub libraries: Vec<Source>,
    pub volumes: Vec<Volume>,
    pub warnings: Vec<String>,
    pub owned_account: Option<String>,
    #[serde(default)]
    pub owned_source: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OwnedLibrary {
    pub steamid: String,
    pub games: Vec<OwnedGame>,
    #[serde(default)]
    pub source: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnedGame {
    pub appid: u32,
    pub name: String,
}
