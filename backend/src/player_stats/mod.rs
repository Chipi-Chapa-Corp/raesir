use crate::{
    details,
    model::{LibraryGame, Report},
};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

mod epic;
mod local;
mod steam;
mod ubisoft;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayerStats {
    pub account: String,
    pub source: String,
    pub playtime_seconds: Option<u64>,
    pub achievements: Vec<Achievement>,
    pub total: Option<usize>,
    pub unlocked_count: Option<usize>,
    pub partial: bool,
    pub updated: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Achievement {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub unlocked: bool,
    #[serde(default)]
    pub hidden: bool,
    pub unlocked_at: Option<u64>,
    /// Fraction completed; unknown progress is distinct from zero.
    pub progress: Option<f64>,
    /// Percentage of players who unlocked this achievement.
    pub rarity: Option<f64>,
}

impl Achievement {
    pub fn description_text(&self) -> Option<&str> {
        let text = self.description.trim();
        (!text.is_empty() && !text.split_whitespace().eq(self.name.split_whitespace()))
            .then_some(text)
    }
    pub fn concealed(&self) -> bool {
        self.hidden && !self.unlocked
    }
    pub fn completion(&self) -> f64 {
        if self.unlocked {
            1.
        } else {
            self.progress
                .filter(|p| p.is_finite())
                .unwrap_or(0.)
                .clamp(0., 1.)
        }
    }
}

impl PlayerStats {
    pub fn has_data(&self) -> bool {
        self.playtime_seconds.is_some() || !self.achievements.is_empty()
    }
    pub fn by_progress(&self) -> Vec<usize> {
        let mut indices: Vec<_> = (0..self.achievements.len()).collect();
        indices.sort_by(|&a, &b| {
            let (a, b) = (&self.achievements[a], &self.achievements[b]);
            b.completion()
                .total_cmp(&a.completion())
                .then_with(|| rarity_order(a, b))
                .then_with(|| a.name.cmp(&b.name))
        });
        indices
    }
    pub fn featured(&self) -> Option<usize> {
        let earned = self.achievements.iter().any(|a| a.unlocked);
        (0..self.achievements.len())
            .filter(|&i| {
                !self.achievements[i].concealed() && (!earned || self.achievements[i].unlocked)
            })
            .min_by(|&i, &j| {
                let (a, b) = (&self.achievements[i], &self.achievements[j]);
                rarity_order(a, b)
                    .then_with(|| b.unlocked_at.cmp(&a.unlocked_at))
                    .then_with(|| b.completion().total_cmp(&a.completion()))
                    .then_with(|| a.name.cmp(&b.name))
            })
    }
    pub fn highlights(&self) -> Vec<usize> {
        let mut indices = self.by_progress();
        indices.sort_by(|&a, &b| {
            let (a, b) = (&self.achievements[a], &self.achievements[b]);
            b.unlocked
                .cmp(&a.unlocked)
                .then_with(|| {
                    if a.unlocked {
                        rarity_order(a, b)
                    } else {
                        b.completion()
                            .total_cmp(&a.completion())
                            .then_with(|| rarity_order(a, b))
                    }
                })
                .then_with(|| b.unlocked_at.cmp(&a.unlocked_at))
                .then_with(|| a.name.cmp(&b.name))
        });
        indices.retain(|&i| !self.achievements[i].concealed());
        indices
    }
}
fn rarity_order(a: &Achievement, b: &Achievement) -> Ordering {
    a.rarity
        .unwrap_or(f64::INFINITY)
        .total_cmp(&b.rarity.unwrap_or(f64::INFINITY))
}

fn cache_path(game: &LibraryGame, account: &str, root: &Path) -> PathBuf {
    cache_path_version(
        game,
        account,
        root,
        if game.store == "steam" { 2 } else { 1 },
    )
}
fn cache_path_version(game: &LibraryGame, account: &str, root: &Path, version: i32) -> PathBuf {
    let mut hash = DefaultHasher::new();
    (version, &game.key, account).hash(&mut hash);
    root.join(format!("{:016x}.json", hash.finish()))
}
fn json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&read(path, 8 * 1024 * 1024)?).ok()
}
fn read(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut data = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() as u64 <= limit).then_some(data)
}
fn client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("ExposedGames/0.1")
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(10))
        .build()
        .ok()
}
fn response(response: reqwest::blocking::Response) -> Option<serde_json::Value> {
    let mut data = Vec::new();
    response
        .error_for_status()
        .ok()?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .ok()?;
    (data.len() <= 8 * 1024 * 1024)
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.parse().ok())
        .filter(|n| n.is_finite())
}

pub fn cached(game: &LibraryGame, report: &Report, root: &Path) -> PlayerStats {
    let mut stats = match game.store.as_str() {
        "steam" => steam::cached(game, report),
        "epic" => epic::cached(game, report),
        "ubisoft" => ubisoft::cached(game, report, root),
        "ea" => local::ea(game, report),
        _ => PlayerStats::default(),
    };
    if stats.playtime_seconds.is_none() {
        stats.playtime_seconds = local::playtime(game, report);
    }
    if stats.account.is_empty() {
        return stats;
    }
    let stored = read(&cache_path(game, &stats.account, root), 8 * 1024 * 1024)
        .map(|b| (b, false))
        .or_else(|| {
            if game.store != "steam" {
                return None;
            }
            read(
                &cache_path_version(game, &stats.account, root, 1),
                8 * 1024 * 1024,
            )
            .map(|b| (b, true))
        });
    if !stats.account.is_empty()
        && let Some((data, legacy)) = stored
        && let Ok(mut saved) = serde_json::from_slice::<PlayerStats>(&data)
        && saved.account == stats.account
    {
        for local in &mut stats.achievements {
            if let Some(a) = saved.achievements.iter_mut().find(|a| {
                if game.store == "steam" {
                    a.id.eq_ignore_ascii_case(&local.id)
                } else {
                    a.id == local.id
                }
            }) {
                local.rarity = local.rarity.or(a.rarity);
                a.hidden |= local.hidden;
                local.hidden |= a.hidden;
                if local.unlocked {
                    a.unlocked = true;
                    a.unlocked_at = local.unlocked_at.or(a.unlocked_at);
                }
                if local.progress.is_some() {
                    a.progress = local.progress;
                }
            }
        }
        if !saved.achievements.is_empty() && (stats.partial || stats.achievements.is_empty()) {
            stats.achievements = saved.achievements;
            stats.total = saved.total;
            stats.unlocked_count = Some(
                saved
                    .unlocked_count
                    .unwrap_or(0)
                    .max(stats.achievements.iter().filter(|a| a.unlocked).count()),
            );
            stats.partial = saved.partial;
        }
        stats.playtime_seconds = stats.playtime_seconds.or(saved.playtime_seconds);
        stats.updated = if legacy { 0 } else { saved.updated };
    }
    if game.store == "steam" {
        steam::apply_hidden(game, report, &mut stats);
    }
    stats
}

pub fn refresh(game: &LibraryGame, report: &Report, root: &Path) -> PlayerStats {
    let mut stats = cached(game, report, root);
    if std::env::var_os("EXPOSED_GAMES_DISABLE_NETWORK").is_some()
        || (stats.updated > 0 && details::now().saturating_sub(stats.updated) < 900)
    {
        return stats;
    }
    let refreshed = match game.store.as_str() {
        "steam" => steam::refresh(game, &mut stats),
        "epic" => epic::refresh(game, report, &mut stats),
        _ => false,
    };
    if refreshed && !stats.account.is_empty() {
        stats.updated = details::now();
        if fs::create_dir_all(root).is_ok()
            && let Ok(mut file) = tempfile::NamedTempFile::new_in(root)
            && serde_json::to_writer(&mut file, &stats).is_ok()
        {
            let _ = file.persist(cache_path(game, &stats.account, root));
        }
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secret_locked_achievements_do_not_leak_into_previews() {
        let stats = PlayerStats {
            achievements: vec![
                Achievement {
                    name: "Secret".into(),
                    hidden: true,
                    ..Default::default()
                },
                Achievement {
                    name: "Earned secret".into(),
                    hidden: true,
                    unlocked: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(stats.featured(), Some(1));
        assert_eq!(stats.highlights(), vec![1]);
        assert_eq!(stats.by_progress().len(), 2);
        let legacy: Achievement = serde_json::from_value(serde_json::json!({
            "id":"a","name":"A","description":"","icon":"","unlocked":false,
            "unlocked_at":null,"progress":null,"rarity":null
        }))
        .unwrap();
        assert!(!legacy.hidden);
    }
    #[test]
    fn featured_uses_rarest_earned_achievement_or_latest_unlock_without_rarity() {
        let mut s = PlayerStats {
            achievements: vec![
                Achievement {
                    unlocked: true,
                    unlocked_at: Some(100),
                    ..Default::default()
                },
                Achievement {
                    unlocked: true,
                    unlocked_at: Some(200),
                    ..Default::default()
                },
                Achievement {
                    rarity: Some(0.1),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(s.featured(), Some(1));
        s.achievements[0].rarity = Some(1.);
        s.achievements[1].rarity = Some(20.);
        assert_eq!(s.featured(), Some(0));
        s.achievements[0].unlocked = false;
        s.achievements[1].unlocked = false;
        assert_eq!(s.featured(), Some(2));
    }
    #[test]
    fn repeated_achievement_names_are_not_rendered_as_descriptions() {
        let mut a = Achievement {
            name: "Reached Coal Pits".into(),
            description: " Reached  Coal\nPits ".into(),
            ..Default::default()
        };
        assert_eq!(a.description_text(), None);
        a.description = " Find the entrance to the mines ".into();
        assert_eq!(a.description_text(), Some("Find the entrance to the mines"));
    }
    #[test]
    fn rarity_previews_and_progress_order_are_independent() {
        assert_eq!(number(&serde_json::json!("12.5")), Some(12.5));
        assert_eq!(number(&serde_json::json!("NaN")), None);
        let stats = PlayerStats {
            achievements: vec![
                Achievement {
                    name: "common".into(),
                    unlocked: true,
                    rarity: Some(90.),
                    ..Default::default()
                },
                Achievement {
                    name: "rare".into(),
                    unlocked: true,
                    rarity: Some(1.2),
                    ..Default::default()
                },
                Achievement {
                    name: "almost".into(),
                    progress: Some(0.9),
                    rarity: Some(0.1),
                    ..Default::default()
                },
                Achievement {
                    name: "started".into(),
                    progress: Some(0.1),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(stats.highlights(), vec![1, 0, 2, 3]);
        assert_eq!(stats.by_progress(), vec![1, 0, 2, 3]);
        assert_ne!(
            cache_path(
                &crate::catalog::entry("steam", "1", "", vec![], ""),
                "a",
                Path::new("/")
            ),
            cache_path(
                &crate::catalog::entry("steam", "1", "", vec![], ""),
                "b",
                Path::new("/")
            )
        );
        assert!(!PlayerStats::default().has_data());
        assert!(
            PlayerStats {
                playtime_seconds: Some(0),
                ..Default::default()
            }
            .has_data()
        );
    }
}
