//! Device connections: a board or instrument plugged into the person's computer,
//! reached through their browser. Chat never touches the device. Each tool call is
//! written to `device_calls`; the browser tab showing the chat carries it out over
//! Web Serial, posts the result, and the response goes on. The serial monitor,
//! chip identification and flashing all live in the browser (ui/lib/devices).
use super::mcp::Tool;
use crate::error::ApiError;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::time::Duration;
use uuid::Uuid;

/// How long a call waits for the browser. A flash at a slow baud rate takes minutes,
/// and the person may first have to plug the device in.
pub const CALL_WAIT: Duration = Duration::from_secs(10 * 60);
/// What the connections table holds as the address of a device: there is none.
pub const URL: &str = "browser://device";
/// The transports a device connection can name. Serial covers USB serial adapters
/// and boards with native USB CDC; more come with the browser's own support.
pub const TRANSPORTS: &[&str] = &["serial"];
/// The tool every chat gets while devices are offered: asks the person to attach one.
pub const REQUEST: &str = "request_device";
pub const REQUEST_SHOWN: &str = "The person sees a button to attach a device from their computer. End your reply now in at most one short sentence, without repeating the reason; once they attach it, their next message continues the task and you will have the device's tools.";

pub fn request_definition() -> Value {
    json!({"type":"function","function":{"name":REQUEST,
        "description":"Ask the person to attach a device plugged into their computer: a development board, a serial adapter, an instrument. Through their browser you can then open its serial port, read what it prints, send it commands, reset it, identify an Espressif chip and flash firmware built in the workspace. Call it when the person wants to work with hardware at their computer and you have no tools for that device. Your reply ends with a button; once they attach it, their next message continues the task with the device's tools.",
        "parameters":{"type":"object","properties":{
            "reason":{"type":"string","description":"Why, in one short sentence the person reads, such as \"To read what the BeagleBone prints over its console.\""},
            "name":{"type":"string","description":"A name for the device, as the person called it, such as \"BeagleBone\""}},
        "required":["reason"],"additionalProperties":false}}})
}

/// The card at the end of the reply that asks for a device.
pub fn card_entry(call_id: &str, arguments: &str) -> Value {
    let args: Value = serde_json::from_str(arguments).unwrap_or(json!({}));
    let text = |key: &str, limit: usize| -> String {
        args[key]
            .as_str()
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(limit)
            .collect::<String>()
            .trim()
            .to_owned()
    };
    json!({"id":call_id,"kind":"device-request","tool":REQUEST,"status":"asked","summary":"Asked for a device","reason":text("reason", 240),"name":text("name", 40)})
}

/// The longest result a browser may post.
const RESULT_LIMIT: usize = 48 * 1024;
const DETAIL_LIMIT: usize = 4 * 1024;

fn tool(
    name: &str,
    title: &str,
    description: &str,
    read_only: bool,
    properties: Value,
    required: &[&str],
) -> Tool {
    Tool {
        name: name.into(),
        title: Some(title.into()),
        description: description.into(),
        input_schema: json!({"type":"object","properties":properties,"required":required}),
        read_only,
    }
}

/// A device's tools, named by what they do over the transport, never by a board.
pub fn tools() -> Vec<Tool> {
    vec![
        tool("status", "Device status", "Whether the device is attached in the person's browser, whether its port is open and at what baud rate, and how much unread output waits.", true, json!({}), &[]),
        tool("open", "Open port", "Opens the device's serial port and starts collecting what it prints. Open before reading or writing; open again to change the baud rate.", true, json!({
            "baudRate":{"type":"integer","minimum":300,"maximum":4000000,"description":"Defaults to 115200"},
            "dataBits":{"type":"integer","enum":[7,8]},
            "stopBits":{"type":"integer","enum":[1,2]},
            "parity":{"type":"string","enum":["none","even","odd"]},
            "flowControl":{"type":"string","enum":["none","hardware"]}
        }), &[]),
        tool("close", "Close port", "Closes the device's serial port.", true, json!({}), &[]),
        tool("read", "Read output", "Text the device printed since the last read. Waits up to waitMs for more to arrive, or until the text `until` appears.", true, json!({
            "waitMs":{"type":"integer","minimum":0,"maximum":60000,"description":"Defaults to 2000"},
            "until":{"type":"string","description":"Return as soon as this text has arrived"},
            "maxChars":{"type":"integer","minimum":100,"maximum":16000,"description":"Defaults to 6000; the end of the output is kept"}
        }), &[]),
        tool("write", "Send to device", "Sends text to the device, followed by a newline unless newline is false: a line for a REPL, an AT command for a modem, a command for your own firmware. Read afterwards to see the answer.", false, json!({
            "text":{"type":"string"},
            "newline":{"type":"boolean","description":"Append a newline (default true)"},
            "hex":{"type":"boolean","description":"text is hex bytes such as 'A5 01 FF' rather than characters"}
        }), &["text"]),
        tool("signals", "Reset or set signals", "Drives the port's DTR and RTS lines. pulse 'reset' restarts the board the way its serial adapter wires reset; pulse 'bootloader' holds the boot pin while resetting, so the board waits for a flasher. Or set dtr and rts directly.", false, json!({
            "pulse":{"type":"string","enum":["reset","bootloader"]},
            "dtr":{"type":"boolean"},
            "rts":{"type":"boolean"}
        }), &[]),
        tool("login", "Sign in at the console", "Signs in at the device's console: wakes it, answers the login prompt with the user name and the password prompt with a saved secret's value, which you never see, and reads the prompt that follows. Call it before running commands on a board that runs Linux. The device's saved user name and secret are used unless you name others. Opens the port at 115200 if it is closed.", true, json!({
            "user":{"type":"string","description":"The user name, if not the one saved for the device"},
            "secret":{"type":"string","description":"The name of the person's secret holding the password, if not the one saved for the device"}
        }), &[]),
        tool("identify", "Identify chip", "Asks the chip's ROM bootloader what it is: chip family and revision, MAC address, flash size and features. Supported today: Espressif ESP32 chips. Pauses the monitor and restarts the board afterwards.", true, json!({}), &[]),
        tool("flash", "Write firmware", "Writes firmware images from the chat's workspace to the chip's flash at the given offsets, then restarts the board. ESP-IDF builds list the offsets in build/flasher_args.json; Arduino and PlatformIO builds put the bootloader at 0x0 (ESP32-S3, C3) or 0x1000 (ESP32), the partition table at 0x8000, boot_app0 at 0xe000 and the app at 0x10000. Supported today: Espressif chips. The workspace must be running.", false, json!({
            "parts":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","properties":{
                "offset":{"type":"string","description":"Flash address, hex such as 0x10000"},
                "path":{"type":"string","description":"The image's path in the workspace"}
            },"required":["offset","path"]}},
            "eraseAll":{"type":"boolean","description":"Erase the whole flash first"},
            "baudRate":{"type":"integer","minimum":115200,"maximum":2000000,"description":"Defaults to 460800"}
        }), &["parts"]),
        tool("erase", "Erase flash", "Erases the chip's whole flash. Supported today: Espressif chips.", false, json!({}), &[]),
    ]
}

/// What the person is asked to allow, read from the arguments.
pub fn changes(tool: &str, args: &Value) -> Result<Vec<Value>, String> {
    match tool {
        "write" => {
            let text = args["text"].as_str().unwrap_or("");
            if text.is_empty() || text.len() > 64 * 1024 {
                return Err("text must be between 1 byte and 64 KB".into());
            }
            let shown: String = text.chars().take(120).collect();
            let what = if shown.len() < text.len() {
                format!("{shown}…")
            } else {
                shown
            };
            let mut facts = vec![];
            if args["hex"] == true {
                facts.push(json!(["As", "hex bytes"]));
            }
            if args["newline"] == false {
                facts.push(json!(["Newline", "none"]));
            }
            Ok(vec![json!({"verb":"Send","what":what,"facts":facts})])
        }
        "signals" => {
            let pulse = args["pulse"].as_str();
            let dtr = args["dtr"].as_bool();
            let rts = args["rts"].as_bool();
            if pulse.is_none() && dtr.is_none() && rts.is_none() {
                return Err("Name a pulse, or dtr or rts".into());
            }
            let (verb, what) = match pulse {
                Some("reset") => ("Reset", "the board".to_owned()),
                Some("bootloader") => ("Reset", "the board into its bootloader".to_owned()),
                _ => (
                    "Set",
                    [("DTR", dtr), ("RTS", rts)]
                        .into_iter()
                        .filter_map(|(line, value)| {
                            value.map(|v| format!("{line} {}", if v { "high" } else { "low" }))
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            };
            Ok(vec![json!({"verb":verb,"what":what,"facts":[]})])
        }
        "flash" => {
            let parts = args["parts"]
                .as_array()
                .filter(|p| !p.is_empty() && p.len() <= 16)
                .ok_or("parts must list 1 to 16 images")?;
            let mut facts = vec![];
            for part in parts {
                let offset = part["offset"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| part["offset"].as_u64().map(|o| format!("{o:#x}")))
                    .filter(|o| parse_offset(o).is_some())
                    .ok_or("Each part needs a flash offset such as 0x10000")?;
                let path = part["path"]
                    .as_str()
                    .map(str::trim)
                    .filter(|p| !p.is_empty() && !p.starts_with('/') && !p.contains(".."))
                    .ok_or("Each part needs a path in the workspace")?;
                facts.push(json!([offset, path]));
            }
            if args["eraseAll"] == true {
                facts.push(json!(["First", "erase the whole flash"]));
            }
            Ok(vec![
                json!({"verb":"Flash","what":format!("{} image{}", parts.len(), if parts.len() == 1 { "" } else { "s" }),"facts":facts}),
            ])
        }
        "erase" => Ok(vec![
            json!({"verb":"Erase","what":"the whole flash","facts":[]}),
        ]),
        _ => Ok(vec![]),
    }
}

/// A flash offset as the model writes it: hex with 0x, or decimal.
pub fn parse_offset(text: &str) -> Option<u64> {
    let text = text.trim();
    match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => text.parse().ok(),
    }
    .filter(|o| *o < 1 << 32)
}

/// The options a device connection keeps: its transport, the USB ids the browser
/// read off the port, and the port's own label.
pub fn clean_options(options: &Value) -> Result<Value, ApiError> {
    let transport = options["transport"].as_str().unwrap_or("serial");
    if !TRANSPORTS.contains(&transport) {
        return Err(ApiError::bad("The transport must be serial"));
    }
    let mut clean = json!({"transport": transport});
    for key in ["vendorId", "productId"] {
        match &options[key] {
            Value::Null => {}
            Value::Number(n) if n.as_u64().is_some_and(|v| v <= 0xffff) => {
                clean[key] = options[key].clone();
            }
            _ => {
                return Err(ApiError::bad(format!(
                    "{key} must be a USB id (0 to 65535)"
                )))
            }
        }
    }
    for (key, what) in [("user", "user name"), ("secret", "secret name")] {
        if let Some(value) = options[key]
            .as_str()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            let ok = if key == "secret" {
                super::vault::valid_name(value)
            } else {
                value.chars().count() <= 63
                    && !value.chars().any(|c| c.is_control() || c.is_whitespace())
            };
            if !ok {
                return Err(ApiError::bad(format!(
                    "The {what} is too long or has spaces"
                )));
            }
            clean[key] = json!(value);
        }
    }
    if let Some(label) = options["label"]
        .as_str()
        .map(str::trim)
        .filter(|l| !l.is_empty())
    {
        if label.chars().count() > 120 {
            return Err(ApiError::bad("The label can be at most 120 characters"));
        }
        clean["label"] = json!(label);
    }
    Ok(clean)
}

/// What the model is told about the device.
pub fn about(options: &Value) -> String {
    let mut about = match options["label"].as_str() {
        Some(label) => format!(
            "a device plugged into the person's computer ({label}), reached through their browser"
        ),
        None => "a device plugged into the person's computer, reached through their browser".into(),
    };
    match (options["user"].as_str(), options["secret"].as_str()) {
        (Some(user), Some(secret)) => about.push_str(&format!(
            "; its console sign-in is saved (user {user}, password in the secret {secret}), so call login before running commands"
        )),
        _ => about.push_str("; no console sign-in is saved for it, so never guess passwords: if its console asks for one, ask the person to save it as a secret and set it under Compute › Devices"),
    }
    about
}

/// Sends one call to the browser and waits for its result.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    pool: &PgPool,
    owner: Uuid,
    conversation: Uuid,
    generation: Uuid,
    connection: Uuid,
    call: &str,
    tool: &str,
    arguments: &Value,
) -> Result<(bool, String), String> {
    let _ = sqlx::query("DELETE FROM device_calls WHERE created_at < NOW() - INTERVAL '1 day'")
        .execute(pool)
        .await;
    sqlx::query("INSERT INTO device_calls(generation_id,call_id,conversation_id,owner_id,connection_id,tool_name,arguments) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT (generation_id,call_id) DO UPDATE SET ok=NULL,output=NULL,detail=NULL,connection_id=EXCLUDED.connection_id,tool_name=EXCLUDED.tool_name,arguments=EXCLUDED.arguments,created_at=NOW()")
        .bind(generation).bind(call).bind(conversation).bind(owner).bind(connection).bind(tool).bind(arguments)
        .execute(pool).await.map_err(|e| format!("The call could not be sent to the browser: {e}"))?;
    let deadline = tokio::time::Instant::now() + CALL_WAIT;
    while tokio::time::Instant::now() < deadline {
        let done: Result<Option<(Option<bool>, Option<String>)>, _> = sqlx::query_as(
            "SELECT ok, output FROM device_calls WHERE generation_id=$1 AND call_id=$2",
        )
        .bind(generation)
        .bind(call)
        .fetch_optional(pool)
        .await;
        match done {
            Ok(Some((Some(ok), output))) => {
                let _ =
                    sqlx::query("DELETE FROM device_calls WHERE generation_id=$1 AND call_id=$2")
                        .bind(generation)
                        .bind(call)
                        .execute(pool)
                        .await;
                return Ok((ok, output.unwrap_or_default()));
            }
            Ok(Some(_)) => {}
            // The row is gone: the chat was deleted.
            Ok(None) => return Err("The call was dropped".into()),
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = sqlx::query("DELETE FROM device_calls WHERE generation_id=$1 AND call_id=$2")
        .bind(generation)
        .bind(call)
        .execute(pool)
        .await;
    Err("No browser carried this out within 10 minutes. The device must be attached in the browser tab that shows this chat.".into())
}

pub async fn finish(pool: &PgPool, generation: Uuid) {
    let _ = sqlx::query("DELETE FROM device_calls WHERE generation_id=$1")
        .bind(generation)
        .execute(pool)
        .await;
}

async fn owned(pool: &PgPool, user: Uuid, conversation: Uuid) -> Result<(), ApiError> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=$1 AND owner_id=$2)",
    )
    .bind(conversation)
    .bind(user)
    .fetch_one(pool)
    .await?;
    if found {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// The calls of a chat still waiting for a browser, oldest first, so a page that
/// opens or reloads while a response runs can pick them up.
pub async fn pending(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
    Path(conversation): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    let user = s.auth.require(&headers, false).await?;
    owned(&s.store.pool, user.id, conversation).await?;
    let rows: Vec<(Uuid, String, Uuid, String, Value, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT generation_id,call_id,connection_id,tool_name,arguments,created_at FROM device_calls WHERE conversation_id=$1 AND ok IS NULL ORDER BY created_at",
    )
    .bind(conversation)
    .fetch_all(&s.store.pool)
    .await?;
    let calls: Vec<Value> = rows
        .into_iter()
        .map(|(generation, call, connection, tool, arguments, created)| {
            json!({"generationId":generation,"callId":call,"connectionId":connection,"tool":tool,"arguments":arguments,"createdAt":created})
        })
        .collect();
    Ok(Json(json!({"calls": calls})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Result_ {
    ok: bool,
    #[serde(default)]
    output: String,
    detail: Option<String>,
}

/// The browser's result for one call. The output is cut to what the model reads.
pub async fn resolve(
    State(s): State<crate::AppState>,
    headers: HeaderMap,
    Path((conversation, call)): Path<(Uuid, String)>,
    Json(body): Json<Result_>,
) -> Result<StatusCode, ApiError> {
    let user = s.auth.require(&headers, true).await?;
    let output = clip(&body.output, RESULT_LIMIT);
    let detail = body.detail.as_deref().map(|d| clip(d, DETAIL_LIMIT));
    let done = sqlx::query("UPDATE device_calls SET ok=$4,output=$5,detail=$6 WHERE conversation_id=$1 AND call_id=$2 AND owner_id=$3 AND ok IS NULL")
        .bind(conversation).bind(&call).bind(user.id).bind(body.ok).bind(output).bind(detail)
        .execute(&s.store.pool).await?;
    if done.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This step is no longer waiting for the device".into(),
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… ({} more bytes)", &text[..end], text.len() - end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_card_carries_the_reason_and_a_name() {
        let card = card_entry(
            "c1",
            r#"{"reason":"To read the console.","name":"Beagle\u0007Bone"}"#,
        );
        assert_eq!(card["kind"], "device-request");
        assert_eq!(card["reason"], "To read the console.");
        assert_eq!(card["name"], "BeagleBone");
        assert_eq!(card_entry("c2", "nonsense")["reason"], "");
        assert_eq!(request_definition()["function"]["name"], REQUEST);
    }

    #[test]
    fn offsets_read_hex_and_decimal() {
        assert_eq!(parse_offset("0x10000"), Some(0x10000));
        assert_eq!(parse_offset("0X0"), Some(0));
        assert_eq!(parse_offset("65536"), Some(65536));
        assert_eq!(parse_offset("0x100000000"), None);
        assert_eq!(parse_offset("boot"), None);
    }

    #[test]
    fn a_flash_review_names_every_image_and_refuses_bad_paths() {
        let items = changes(
            "flash",
            &json!({"parts":[{"offset":"0x0","path":"build/bootloader/bootloader.bin"},{"offset":"0x10000","path":"build/app.bin"}],"eraseAll":true}),
        )
        .unwrap();
        assert_eq!(items[0]["verb"], "Flash");
        assert_eq!(items[0]["what"], "2 images");
        assert_eq!(items[0]["facts"][1], json!(["0x10000", "build/app.bin"]));
        assert_eq!(
            items[0]["facts"][2],
            json!(["First", "erase the whole flash"])
        );
        assert!(changes(
            "flash",
            &json!({"parts":[{"offset":"0x0","path":"../etc/x"}]})
        )
        .is_err());
        assert!(changes("flash", &json!({"parts":[{"offset":"app","path":"a.bin"}]})).is_err());
        assert!(changes("flash", &json!({"parts":[]})).is_err());
    }

    #[test]
    fn a_write_review_shows_the_text_and_signals_need_a_choice() {
        let items = changes("write", &json!({"text":"AT+GMR","newline":false})).unwrap();
        assert_eq!(items[0]["verb"], "Send");
        assert_eq!(items[0]["what"], "AT+GMR");
        assert_eq!(items[0]["facts"][0], json!(["Newline", "none"]));
        assert!(changes("signals", &json!({})).is_err());
        assert_eq!(
            changes("signals", &json!({"pulse":"bootloader"})).unwrap()[0]["what"],
            "the board into its bootloader"
        );
        assert_eq!(
            changes("signals", &json!({"dtr":false,"rts":true})).unwrap()[0]["what"],
            "DTR low, RTS high"
        );
    }

    #[test]
    fn options_keep_the_transport_ids_and_label() {
        let clean = clean_options(&json!({"vendorId":0x303a,"productId":4097,"label":"USB JTAG/serial debug unit","extra":1})).unwrap();
        assert_eq!(
            clean,
            json!({"transport":"serial","vendorId":0x303a,"productId":4097,"label":"USB JTAG/serial debug unit"})
        );
        assert!(clean_options(&json!({"transport":"usb"})).is_err());
        assert!(clean_options(&json!({"vendorId":70000})).is_err());
        assert_eq!(
            clean_options(&json!({})).unwrap(),
            json!({"transport":"serial"})
        );
        assert_eq!(
            clean_options(&json!({"user":" debian ","secret":"bb-pass"})).unwrap(),
            json!({"transport":"serial","user":"debian","secret":"bb-pass"})
        );
        assert!(clean_options(&json!({"user":"de bian"})).is_err());
        assert!(clean_options(&json!({"secret":"no spaces"})).is_err());
        assert!(about(&json!({"user":"debian","secret":"bb-pass"}))
            .contains("user debian, password in the secret bb-pass"));
        assert!(about(&json!({})).contains("never guess passwords"));
    }
}
