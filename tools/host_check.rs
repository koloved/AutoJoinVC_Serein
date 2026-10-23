//! Run this plugin's built Wasm through the Serein host sandbox.
//!
//! Copy into `crates/extensions/examples/` of a Serein checkout and run:
//!   cargo run --locked -p extensions --example auto_join_vc_check -- <manifest.json> <module.wasm>
//!
//! It verifies the manifest with the same rules package import uses, then executes the
//! real module: panel shape, effect capabilities and activation behaviour.
use extensions::{AppAction, AppSnapshot, HostEffect, Invocation, Manifest, Package, invoke};
use serde_json::json;
use std::{env, error::Error, fs};

fn check<T>(result: Result<T, extensions::Error>) -> Result<T, Box<dyn Error>> {
	result.map_err(|error| format!("{error:?}").into())
}

fn snapshot(in_call: bool) -> Result<AppSnapshot, Box<dyn Error>> {
	let voice = if in_call {
		json!({
			"channel_id": "20", "phase": "connected", "muted": false, "deafened": false,
			"camera": false, "streaming": false, "participants": []
		})
	} else {
		json!({
			"phase": "idle", "muted": false, "deafened": false,
			"camera": false, "streaming": false, "participants": []
		})
	};
	Ok(serde_json::from_value(json!({
		"context": {"connected": true},
		"guilds": {"items": [{"id": "10", "name": "Home"}], "truncated": false},
		"channels": {"items": [
			{"id": "20", "guild_id": "10", "name": "general", "kind": 2},
			{"id": "21", "guild_id": "10", "name": "stage", "kind": 13},
			{"id": "30", "guild_id": "10", "name": "chat", "kind": 0}
		], "truncated": false},
		"voice": voice
	}))?)
}

fn main() -> Result<(), Box<dyn Error>> {
	let mut args = env::args_os().skip(1);
	let manifest_path = args.next().ok_or("usage: <manifest.json> <module.wasm>")?;
	let wasm_path = args.next().ok_or("usage: <manifest.json> <module.wasm>")?;
	let manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
	check(manifest.validate())?;
	let package = Package {
		manifest,
		theme: None,
		background_image: Vec::new(),
		cover_image: Vec::new(),
		wasm: fs::read(&wasm_path)?,
	};

	let open = check(invoke(
		&package,
		&Invocation {
			action: "open".into(),
			..Default::default()
		},
	))?;
	assert!(!open.panel.is_empty(), "open returned no panel");
	assert!(open.effects.is_empty(), "open must stay passive");
	println!("open: {} panel elements, no proposal", open.panel.len());

	let mut join = Invocation {
		action: "join".into(),
		app: Some(Box::new(snapshot(false)?)),
		..Default::default()
	};
	join.values
		.insert("channel".into(), "Home — general (20)".into());
	join.values.insert("join-deafened".into(), "true".into());
	let joined = check(invoke(&package, &join))?;
	match joined.effects.as_slice() {
		[HostEffect::AppAction {
			action:
				AppAction::JoinVoice {
					channel_id,
					deafened: true,
					..
				},
		}] if channel_id == "20" => {}
		other => return Err(format!("unexpected join proposal: {other:?}").into()),
	}
	println!("join: proposed joining channel 20 while deafened");

	let leave = check(invoke(
		&package,
		&Invocation {
			action: "leave".into(),
			app: Some(Box::new(snapshot(true)?)),
			..Default::default()
		},
	))?;
	assert!(matches!(
		leave.effects.as_slice(),
		[HostEffect::LeaveVoice]
	));
	println!("leave: proposed leaving the current call");

	let activated = check(invoke(
		&package,
		&Invocation {
			action: "activate".into(),
			storage: Some(r#"{"channel_id":"nope"}"#.into()),
			..Default::default()
		},
	))?;
	assert!(
		activated.effects.is_empty(),
		"activation may not propose a call"
	);
	let stored = activated.storage.as_deref().ok_or("activation saved nothing")?;
	assert!(!stored.contains("nope"), "activation kept an invalid id");
	println!("activate: repaired storage to {stored}");

	println!("Host sandbox accepted the package.");
	Ok(())
}
