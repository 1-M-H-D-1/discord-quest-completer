use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Plan {
    pub steam_id: String,
    pub name: String,
    pub folder: String,
    pub executable: String,
}

#[derive(Clone, Serialize)]
pub struct Launch {
    pub description: String,
    pub plan: Plan,
}

#[derive(Clone, Serialize)]
pub struct Metadata {
    pub steam_id: String,
    pub name: String,
    pub folder: String,
    pub launches: Vec<Launch>,
    pub source: String,
    pub launcher_urls: Vec<String>,
}

pub fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || !id.bytes().all(|b| b.is_ascii_digit())
        || id.starts_with('0')
        || id.parse::<u32>().unwrap_or(0) == 0
    {
        return Err("Invalid Steam application ID.".into());
    }
    Ok(())
}

// Never allow external metadata to address a drive, UNC share, parent directory,
// device name, alternate data stream, or ambiguous Windows filename.
pub fn relative_path(value: &str) -> Result<String, String> {
    let value = value.replace('\\', "/");
    if value.is_empty() || value.len() > 220 || value.starts_with('/') {
        return Err("Steam supplied an unsupported file path.".into());
    }
    for part in value.split('/') {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        let device = matches!(
            stem.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with(['.', ' '])
            || part
                .chars()
                .any(|c| c.is_control() || "<>:\"|?*".contains(c))
            || device
        {
            return Err("Steam supplied an unsafe or unsupported Windows file path.".into());
        }
    }
    Ok(value)
}

impl Plan {
    pub fn validate(&self) -> Result<(), String> {
        valid_id(&self.steam_id)?;
        let folder = relative_path(&self.folder)?;
        if folder.contains('/') || folder != self.folder {
            return Err("Unsupported Steam installation folder.".into());
        }
        let exe = relative_path(&self.executable)?;
        if exe != self.executable || !exe.to_ascii_lowercase().ends_with(".exe") {
            return Err("Steam did not supply a relative Windows EXE path.".into());
        }
        if self.name.is_empty() || self.name.len() > 512 || self.name.chars().any(char::is_control)
        {
            return Err("Invalid Steam game name.".into());
        }
        Ok(())
    }

    pub fn manifest(&self) -> String {
        let name = self.name.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"AppState\"\n{{\n\t\"appid\" \"{}\"\n\t\"name\" \"{}\"\n\t\"StateFlags\" \"4\"\n\t\"installdir\" \"{}\"\n}}\n", self.steam_id, name, self.folder)
    }
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub fn parse_metadata(steam_id: &str, response: &Value) -> Result<Metadata, String> {
    valid_id(steam_id)?;
    if response["status"] != "success" {
        return Err("Steam metadata service returned no app information.".into());
    }
    let app = &response["data"][steam_id];
    let name = text(&app["common"], "name");
    let folder = relative_path(text(&app["config"], "installdir"))?;
    let launch_data = &app["config"]["launch"];
    let entries: Vec<&Value> = match launch_data {
        Value::Object(items) => items.values().collect(),
        Value::Array(items) => items.iter().collect(),
        _ => vec![],
    };
    let mut launches = Vec::<Launch>::new();
    let mut launcher_urls = Vec::new();
    for entry in entries {
        let options = &entry["config"];
        let os = text(options, "oslist");
        let os = if os.is_empty() {
            text(&app["common"], "oslist")
        } else {
            os
        };
        let branch = text(options, "betakey");
        if (!os.is_empty() && !os.split(',').any(|s| s.trim() == "windows"))
            || (!branch.is_empty() && branch != "public")
        {
            continue;
        }
        let raw = text(entry, "executable");
        if raw.contains("://") {
            let scheme = raw.split("://").next().unwrap_or("");
            if !scheme.is_empty()
                && scheme
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b))
            {
                launcher_urls.push(raw.to_string());
            }
            continue;
        }
        // Some publishers prefix an install-relative launch path with one '/'.
        // Normalize that convention without accepting UNC or drive paths.
        let raw = if raw.starts_with('/') && !raw.starts_with("//") {
            &raw[1..]
        } else {
            raw
        };
        let executable = match relative_path(raw) {
            Ok(p) => p,
            Err(_) => continue,
        };
        let plan = Plan {
            steam_id: steam_id.into(),
            name: name.into(),
            folder: folder.clone(),
            executable,
        };
        if plan.validate().is_err() {
            continue;
        }
        if launches
            .iter()
            .any(|l| l.plan.executable.eq_ignore_ascii_case(&plan.executable))
        {
            continue;
        }
        launches.push(Launch {
            description: text(entry, "description").into(),
            plan,
        });
    }
    if launches.is_empty() && launcher_urls.is_empty() {
        return Err("Steam publishes no usable public Windows EXE for this game. It may use a launcher URL, another platform, or private launch entries. No filename was guessed.".into());
    }
    Ok(Metadata {
        steam_id: steam_id.into(),
        name: name.into(),
        folder,
        launches,
        source: format!("https://api.steamcmd.net/v1/info/{}", steam_id),
        launcher_urls,
    })
}

pub async fn request_metadata(steam_id: String) -> Result<Metadata, String> {
    valid_id(&steam_id)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .get(format!("https://api.steamcmd.net/v1/info/{}", steam_id))
        .send()
        .await
        .map_err(|e| format!("Steam metadata lookup failed: {}", e))?
        .error_for_status()
        .map_err(|e| format!("Steam metadata service: {}", e))?;
    let data: Value = response
        .json()
        .await
        .map_err(|e| format!("Invalid Steam metadata response: {}", e))?;
    parse_metadata(&steam_id, &data)
}

#[tauri::command]
pub async fn fetch_steam_metadata(steam_id: String) -> Result<Metadata, String> {
    request_metadata(steam_id).await
}

#[tauri::command]
pub fn validate_manual_steam_plan(mut plan: Plan) -> Result<Plan, String> {
    plan.steam_id = plan.steam_id.trim().to_string();
    plan.name = plan.name.trim().to_string();
    plan.folder = plan.folder.trim().to_string();
    plan.executable = plan.executable.trim().replace('\\', "/");
    plan.validate()?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn manual_details_are_normalized_and_validated() {
        let plan = validate_manual_steam_plan(Plan {
            steam_id: " 123 ".into(),
            name: " Example ".into(),
            folder: " Example Game ".into(),
            executable: " Bin\\Win64\\game.exe ".into(),
        })
        .unwrap();
        assert_eq!(plan.steam_id, "123");
        assert_eq!(plan.folder, "Example Game");
        assert_eq!(plan.executable, "Bin/Win64/game.exe");
        assert_eq!(plan.name, "Example");
    }
    #[test]
    fn manual_details_reject_missing_ids_urls_and_unsafe_paths() {
        let base = Plan {
            steam_id: "123".into(),
            name: "Example".into(),
            folder: "Example".into(),
            executable: "game.exe".into(),
        };
        for executable in [
            "",
            "vendor://launch/123",
            "C:\\game.exe",
            "../game.exe",
            "NUL.exe",
            "game.bat",
        ] {
            let mut plan = base.clone();
            plan.executable = executable.into();
            assert!(validate_manual_steam_plan(plan).is_err());
        }
        let mut missing_id = base.clone();
        missing_id.steam_id = "".into();
        assert!(validate_manual_steam_plan(missing_id).is_err());
        let mut bad_folder = base;
        bad_folder.folder = "../../outside".into();
        assert!(validate_manual_steam_plan(bad_folder).is_err());
    }
    #[tokio::test]
    #[ignore = "Requires network and DQC_STEAM_TEST_APP_ID"]
    async fn live_metadata_lookup() {
        let id = std::env::var("DQC_STEAM_TEST_APP_ID").unwrap();
        let metadata = request_metadata(id.clone()).await.unwrap();
        assert_eq!(metadata.steam_id, id);
        assert!(!metadata.launches.is_empty());
        for launch in &metadata.launches {
            launch.plan.validate().unwrap();
            println!("{} / {}", launch.plan.folder, launch.plan.executable);
        }
    }
    fn response(launch: Value) -> Value {
        json!({"status":"success","data":{"123":{"common":{"name":"Example","oslist":"windows"},"config":{"installdir":"Example Game","launch":launch}}}})
    }
    #[test]
    fn keeps_nested_public_windows_paths_and_deduplicates() {
        let metadata = parse_metadata(
            "123",
            &response(json!({
                "0":{"executable":"/Game/Binaries/Win64/game.exe"},
                "1":{"executable":"Game\\Binaries\\Win64\\game.exe"},
                "2":{"executable":"private.exe","config":{"betakey":"private"}},
                "3":{"executable":"linux.exe","config":{"oslist":"linux"}}
            })),
        )
        .unwrap();
        assert_eq!(metadata.launches.len(), 1);
        assert_eq!(
            metadata.launches[0].plan.executable,
            "Game/Binaries/Win64/game.exe"
        );
    }
    #[test]
    fn rejects_uris_and_dangerous_paths() {
        for path in [
            "../bad.exe",
            "C:/bad.exe",
            "//server/bad.exe",
            "bad.exe:stream",
            "NUL.exe",
            "a/../b.exe",
            "bad./game.exe",
            "a//b.exe",
        ] {
            assert!(
                parse_metadata("123", &response(json!({"0":{"executable":path}}))).is_err(),
                "{}",
                path
            );
        }
    }
    #[test]
    fn keeps_multiple_options_without_running_their_arguments() {
        let result = parse_metadata(
            "123",
            &response(json!([
                {"executable":"game.exe","arguments":"--untrusted"}, {"executable":"dx12/game.exe"}
            ])),
        )
        .unwrap();
        assert_eq!(result.launches.len(), 2);
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("--untrusted"));
    }
    #[test]
    fn validates_app_id_and_folder() {
        assert!(valid_id("../123").is_err());
        assert!(valid_id("4294967296").is_err());
        let mut data = response(json!({"0":{"executable":"game.exe"}}));
        data["data"]["123"]["config"]["installdir"] = json!("../outside");
        assert!(parse_metadata("123", &data).is_err());
    }
    #[test]
    fn launcher_urls_keep_metadata_for_manual_entry() {
        let data = response(json!({"0":{"executable":"vendor://launch/123"}}));
        let metadata = parse_metadata("123", &data).unwrap();
        assert!(metadata.launches.is_empty());
        assert_eq!(metadata.launcher_urls, ["vendor://launch/123"]);
    }
    #[test]
    fn direct_executable_takes_precedence_over_launcher_url() {
        let metadata = parse_metadata(
            "123",
            &response(json!({
                "0":{"executable":"game.exe"}, "1":{"executable":"vendor://launch/123"}
            })),
        )
        .unwrap();
        assert_eq!(metadata.launches.len(), 1);
        assert_eq!(metadata.launches[0].plan.executable, "game.exe");
    }
}
