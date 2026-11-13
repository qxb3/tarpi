use crate::jv::*;
use crate::lorebook::Lorebook;
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct DepthPrompt {
    pub prompt: String,
    pub depth: usize,
    pub role: String,
}

/// The parts of a character card that matter for chatting.
#[derive(Debug, Clone, Default)]
pub struct Character {
    pub avatar: String,
    pub name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub first_mes: String,
    pub mes_example: String,
    pub system_prompt: String,
    pub post_history_instructions: String,
    pub alternate_greetings: Vec<String>,
    pub depth_prompt: Option<DepthPrompt>,
    /// The card's `character_book`, if it has one that can do anything.
    pub lorebook: Option<Lorebook>,
}

impl Character {
    /// Parse the object returned by `/api/characters/get`.
    /// Prefers the v2 `data.*` fields and falls back to the legacy top-level ones.
    pub fn from_value(v: &Value) -> Character {
        let data = v.get("data").cloned().unwrap_or(Value::Null);
        let pick = |key: &str| -> String {
            let d = jstr(&data, key);
            if !d.trim().is_empty() {
                d
            } else {
                jstr(v, key)
            }
        };

        let alternate_greetings = data
            .get("alternate_greetings")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .filter(|x| !x.trim().is_empty())
                    .map(|x| x.to_string())
                    .collect::<Vec<String>>()
            })
            .unwrap_or_default();

        let depth_prompt = data
            .get("extensions")
            .and_then(|e| e.get("depth_prompt"))
            .and_then(|d| {
                let prompt = jstr(d, "prompt");
                if prompt.trim().is_empty() {
                    return None;
                }
                let role = match jstr(d, "role").as_str() {
                    "user" => "user",
                    "assistant" => "assistant",
                    _ => "system",
                };
                Some(DepthPrompt {
                    prompt,
                    depth: ji64(d, "depth", 4).max(0) as usize,
                    role: role.to_string(),
                })
            });

        let mut name = pick("name");
        if name.trim().is_empty() {
            name = "Character".to_string();
        }

        Character {
            avatar: jstr(v, "avatar"),
            name,
            description: pick("description"),
            personality: pick("personality"),
            scenario: pick("scenario"),
            first_mes: pick("first_mes"),
            mes_example: pick("mes_example"),
            system_prompt: jstr(&data, "system_prompt"),
            post_history_instructions: jstr(&data, "post_history_instructions"),
            alternate_greetings,
            depth_prompt,
            lorebook: Lorebook::from_card_data(&data),
        }
    }
}
