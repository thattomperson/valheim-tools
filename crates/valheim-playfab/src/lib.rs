//! The PlayFab REST calls a Valheim crossplay client makes before it can
//! open a Party link: `LoginWithCustomID`, then `FindLobbies` by join code,
//! `JoinLobby` and `GetLobby` to read the Party network descriptor.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Valheim's PlayFab title.
pub const VALHEIM_TITLE_ID: &str = "6E223";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("PlayFab {endpoint} failed: {status} {code}: {message}")]
    Api {
        endpoint: &'static str,
        status: u16,
        code: i64,
        message: String,
    },
    #[error("unexpected PlayFab response from {endpoint}: {detail}")]
    Shape {
        endpoint: &'static str,
        detail: String,
    },
    #[error("no crossplay server found for join code {0}")]
    NoLobby(String),
    #[error("lobby has no NETWORK entry (server not ready?)")]
    NoNetwork,
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityKey {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Type")]
    pub kind: String,
}

/// What `LoginWithCustomID` gives us.
#[derive(Debug, Clone)]
pub struct Session {
    pub playfab_id: String,
    pub entity: EntityKey,
    pub entity_token: String,
}

/// A lobby found by join code.
#[derive(Debug, Clone)]
pub struct LobbySummary {
    pub lobby_id: String,
    pub connection_string: String,
    pub search_data: HashMap<String, String>,
}

impl LobbySummary {
    /// The server's PlayFab entity ID: the Party endpoint we talk to.
    pub fn server_entity_id(&self) -> Option<&str> {
        self.search_data.get("string_key1").map(String::as_str)
    }

    fn created_ticks(&self) -> i64 {
        self.search_data
            .get("string_key9")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

/// `LobbyData["NETWORK"]`, split.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub invitation_id: String,
    pub serialized_descriptor: String,
}

impl NetworkInfo {
    pub fn parse(s: &str) -> Option<Self> {
        let (inv, desc) = s.split_once('|')?;
        Some(Self {
            invitation_id: inv.to_owned(),
            serialized_descriptor: desc.to_owned(),
        })
    }
}

/// Everything needed to connect to a crossplay server.
#[derive(Debug, Clone)]
pub struct JoinTarget {
    pub lobby_id: String,
    pub server_entity_id: String,
    pub network: NetworkInfo,
    pub password_protected: bool,
    pub lobby_data: HashMap<String, String>,
}

/// The `FindLobbies` filter Valheim uses for a join code.
pub fn join_code_filter(code: &str) -> String {
    // Join codes are digits; strip anything that would break the OData string.
    let code: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("string_key4 eq '{code}' and string_key2 eq 'True'")
}

pub struct Client {
    http: reqwest::blocking::Client,
    base: String,
    title_id: String,
    session: Option<Session>,
}

#[derive(Deserialize)]
struct Envelope {
    code: Option<u16>,
    data: Option<Value>,
    #[serde(rename = "errorCode")]
    error_code: Option<i64>,
    #[serde(rename = "errorMessage")]
    error_message: Option<String>,
}

impl Client {
    pub fn new(title_id: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .user_agent(concat!("valheim-tools/", env!("CARGO_PKG_VERSION")))
                .build()?,
            base: format!("https://{title_id}.playfabapi.com"),
            title_id: title_id.to_owned(),
            session: None,
        })
    }

    pub fn valheim() -> Result<Self> {
        Self::new(VALHEIM_TITLE_ID)
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    fn post<T: DeserializeOwned>(&self, endpoint: &'static str, body: Value) -> Result<T> {
        let mut req = self
            .http
            .post(format!("{}{}", self.base, endpoint))
            .json(&body);
        if let Some(s) = &self.session {
            req = req.header("X-EntityToken", &s.entity_token);
        }
        let resp = req.send()?;
        let status = resp.status().as_u16();
        let env: Envelope = resp.json()?;
        if status != 200 || env.code.unwrap_or(status) != 200 {
            return Err(Error::Api {
                endpoint,
                status,
                code: env.error_code.unwrap_or(0),
                message: env.error_message.unwrap_or_default(),
            });
        }
        serde_json::from_value(env.data.unwrap_or(Value::Null)).map_err(|e| Error::Shape {
            endpoint,
            detail: e.to_string(),
        })
    }

    /// Logs in the way every Valheim client does, with the platform user ID
    /// as the custom ID. Creates the PlayFab account on first use.
    pub fn login_with_custom_id(&mut self, custom_id: &str) -> Result<&Session> {
        #[derive(Deserialize)]
        struct Token {
            #[serde(rename = "EntityToken")]
            token: String,
            #[serde(rename = "Entity")]
            entity: EntityKey,
        }
        #[derive(Deserialize)]
        struct Login {
            #[serde(rename = "PlayFabId")]
            playfab_id: String,
            #[serde(rename = "EntityToken")]
            entity_token: Token,
        }
        let login: Login = self.post(
            "/Client/LoginWithCustomID",
            json!({ "TitleId": self.title_id, "CustomId": custom_id, "CreateAccount": true }),
        )?;
        Ok(self.session.insert(Session {
            playfab_id: login.playfab_id,
            entity: login.entity_token.entity,
            entity_token: login.entity_token.token,
        }))
    }

    fn me(&self) -> Result<&Session> {
        self.session.as_ref().ok_or(Error::Shape {
            endpoint: "login",
            detail: "not logged in".into(),
        })
    }

    /// Finds the crossplay server lobby for a join code. If several match,
    /// the newest wins, as in the game.
    pub fn find_lobby_by_join_code(&self, code: &str) -> Result<LobbySummary> {
        #[derive(Deserialize)]
        struct Lobby {
            #[serde(rename = "LobbyId")]
            lobby_id: String,
            #[serde(rename = "ConnectionString")]
            connection_string: String,
            #[serde(rename = "SearchData", default)]
            search_data: HashMap<String, String>,
        }
        #[derive(Deserialize)]
        struct Found {
            #[serde(rename = "Lobbies", default)]
            lobbies: Vec<Lobby>,
        }
        let found: Found = self.post(
            "/Lobby/FindLobbies",
            json!({ "Filter": join_code_filter(code) }),
        )?;
        found
            .lobbies
            .into_iter()
            .map(|l| LobbySummary {
                lobby_id: l.lobby_id,
                connection_string: l.connection_string,
                search_data: l.search_data,
            })
            .max_by_key(LobbySummary::created_ticks)
            .ok_or_else(|| Error::NoLobby(code.to_owned()))
    }

    /// Joins the lobby so its data becomes readable. Returns the lobby ID.
    pub fn join_lobby(&self, connection_string: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Joined {
            #[serde(rename = "LobbyId")]
            lobby_id: String,
        }
        let me = self.me()?;
        let j: Joined = self.post(
            "/Lobby/JoinLobby",
            json!({ "ConnectionString": connection_string, "MemberEntity": me.entity }),
        )?;
        Ok(j.lobby_id)
    }

    pub fn get_lobby_data(&self, lobby_id: &str) -> Result<HashMap<String, String>> {
        #[derive(Deserialize)]
        struct Lobby {
            #[serde(rename = "LobbyData", default)]
            lobby_data: HashMap<String, String>,
        }
        #[derive(Deserialize)]
        struct Got {
            #[serde(rename = "Lobby")]
            lobby: Lobby,
        }
        let g: Got = self.post("/Lobby/GetLobby", json!({ "LobbyId": lobby_id }))?;
        Ok(g.lobby.lobby_data)
    }

    pub fn leave_lobby(&self, lobby_id: &str) -> Result<()> {
        let me = self.me()?;
        let _: Value = self.post(
            "/Lobby/LeaveLobby",
            json!({ "LobbyId": lobby_id, "MemberEntity": me.entity }),
        )?;
        Ok(())
    }

    /// Find, join and read the lobby for `code`. The caller should
    /// [`leave_lobby`](Self::leave_lobby) when done: lobbies cap at 11 members.
    pub fn resolve_join_code(&self, code: &str) -> Result<JoinTarget> {
        let summary = self.find_lobby_by_join_code(code)?;
        let server_entity_id = summary
            .server_entity_id()
            .ok_or(Error::Shape {
                endpoint: "/Lobby/FindLobbies",
                detail: "lobby has no string_key1 (server entity)".into(),
            })?
            .to_owned();
        let lobby_id = self.join_lobby(&summary.connection_string)?;
        let lobby_data = match self.get_lobby_data(&lobby_id) {
            Ok(d) => d,
            Err(e) => {
                let _ = self.leave_lobby(&lobby_id);
                return Err(e);
            }
        };
        let Some(network) = lobby_data
            .get("NETWORK")
            .and_then(|s| NetworkInfo::parse(s))
        else {
            let _ = self.leave_lobby(&lobby_id);
            return Err(Error::NoNetwork);
        };
        Ok(JoinTarget {
            lobby_id,
            server_entity_id,
            network,
            password_protected: lobby_data.get("PASSWORD").map(String::as_str) == Some("True"),
            lobby_data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter() {
        assert_eq!(
            join_code_filter("123456"),
            "string_key4 eq '123456' and string_key2 eq 'True'"
        );
        assert_eq!(
            join_code_filter("12' or 1 eq 1"),
            "string_key4 eq '12or1eq1' and string_key2 eq 'True'"
        );
    }

    #[test]
    fn network_split() {
        let n = NetworkInfo::parse("inv-1|abc|def").unwrap();
        assert_eq!(n.invitation_id, "inv-1");
        assert_eq!(n.serialized_descriptor, "abc|def");
        assert!(NetworkInfo::parse("nopipe").is_none());
    }

    #[test]
    fn newest_lobby_wins() {
        let mk = |id: &str, ticks: &str| LobbySummary {
            lobby_id: id.into(),
            connection_string: String::new(),
            search_data: [("string_key9".to_string(), ticks.to_string())].into(),
        };
        let best = [mk("a", "5"), mk("b", "9"), mk("c", "1")]
            .into_iter()
            .max_by_key(LobbySummary::created_ticks)
            .unwrap();
        assert_eq!(best.lobby_id, "b");
    }
}
