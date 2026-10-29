use crate::model::{OwnedGame, OwnedLibrary};
use serde::Deserialize;
use std::{io::Read, time::Duration};

#[derive(Deserialize)]
struct Response {
    response: LibraryResponse,
}
#[derive(Deserialize)]
struct LibraryResponse {
    game_count: Option<u32>,
    games: Option<Vec<OwnedGame>>,
}

pub fn parse_response(body: &str, steamid: &str) -> Result<OwnedLibrary, String> {
    let response: Response =
        serde_json::from_str(body).map_err(|_| "Steam returned an invalid library response")?;
    let games = match (response.response.games, response.response.game_count) {
        (Some(games), _) => games,
        (None, Some(0)) => Vec::new(),
        _ => return Err("Steam did not expose this library. Check the API key, SteamID64 and the account's Game details privacy setting. The previous owned list has been kept.".into()),
    };
    Ok(OwnedLibrary {
        steamid: steamid.into(),
        games,
        source: "web_api".into(),
    })
}

pub fn fetch(steamid: &str, api_key: &str) -> Result<OwnedLibrary, String> {
    if steamid.len() != 17 || !steamid.bytes().all(|c| c.is_ascii_digit()) {
        return Err("Enter a 17-digit SteamID64.".into());
    }
    if api_key.len() != 32 || !api_key.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err("Enter a 32-character Steam Web API key.".into());
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not create the Steam API connection")?;
    // Never format reqwest errors: their URL would include the API key.
    let response = client
        .get("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/")
        .query(&[
            ("key", api_key),
            ("steamid", steamid),
            ("include_appinfo", "1"),
            ("include_played_free_games", "1"),
        ])
        .send()
        .map_err(|_| "Steam lookup failed or timed out. Check your connection and retry.")?;
    if !response.status().is_success() {
        return Err(format!(
            "Steam lookup returned HTTP {}. Check your API key and retry.",
            response.status().as_u16()
        ));
    }
    let mut body = String::new();
    response
        .take(8 * 1024 * 1024 + 1)
        .read_to_string(&mut body)
        .map_err(|_| "Could not read the Steam response")?;
    if body.len() > 8 * 1024 * 1024 {
        return Err("Steam response exceeded the size limit".into());
    }
    parse_response(&body, steamid)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_library_is_not_treated_as_empty_owned_list() {
        assert!(parse_response(r#"{"response":{}}"#, "account").is_err());
        assert!(
            parse_response(r#"{"response":{"game_count":0}}"#, "account")
                .unwrap()
                .games
                .is_empty()
        );
        assert_eq!(
            parse_response(
                r#"{"response":{"game_count":1,"games":[{"appid":42,"name":"A game"}]}}"#,
                "account"
            )
            .unwrap()
            .games[0]
                .appid,
            42
        );
    }
}
