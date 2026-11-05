//! Bundled, declarative recipes. This catalog never executes downloaded scripts.
use super::*;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Game {
    pub id: String,
    pub steam: String,
    pub installer: String,
    pub modworkshop: String,
    pub nexus: String,
    pub mods: Vec<Recipe>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub categories: Vec<String>,
    pub repository: String,
    pub mod_asset: String,
    pub folder: String,
    #[serde(default)]
    pub companions: HashMap<String, Companion>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Companion {
    pub asset: String,
    pub executable: String,
    pub arguments: Vec<String>,
    #[serde(default)]
    pub launcher: CompanionLauncher,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompanionLauncher {
    #[default]
    Direct,
    SteamProton,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    version: u32,
    games: Vec<Game>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
    let catalog: Catalog = serde_json::from_str(include_str!("../../data/mod-recipes.json"))
        .expect("valid bundled mod recipes");
    assert_eq!(catalog.version, 1);
    for game in &catalog.games {
        assert!(safe_name(&game.id) && game.installer == "noita");
        for recipe in &game.mods {
            assert!(safe_name(&recipe.id) && safe_name(&recipe.folder));
            assert!(recipe.repository.split('/').count() == 2);
            for companion in recipe.companions.values() {
                assert!(safe_name(&companion.executable));
                assert!(companion.arguments.iter().all(|a| !a.contains('{')
                    || matches!(
                        a.as_str(),
                        "{game.executable}" | "{profile.config}" | "{profile.state}"
                    )));
            }
        }
    }
    catalog
});

pub(super) fn safe_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
}

pub fn for_game(game: &LibraryGame) -> Option<&'static Game> {
    CATALOG
        .games
        .iter()
        .find(|g| game.store == "steam" && game.id == g.steam)
}

pub fn by_id(id: &str) -> Option<&'static Game> {
    CATALOG.games.iter().find(|g| g.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_catalog_matches_exact_game_and_has_complete_host_recipes() {
        let game = crate::catalog::entry("steam", "881100", "Different title", vec![], "test");
        let recipe = for_game(&game).unwrap();
        assert_eq!(recipe.id, "noita");
        let ew = &recipe.mods[0];
        assert_eq!(ew.folder, "quant.ew");
        assert!(ew.companions.contains_key("linux"));
        assert!(ew.companions.contains_key("windows"));
        assert!(!safe_name("../escape"));
        assert!(!safe_name(".."));
    }
}
