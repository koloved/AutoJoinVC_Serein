//! AutoJoinVC for Serein.
//!
//! The Dorion/Shelter original dispatched `VOICE_CHANNEL_SELECT` through the Discord
//! client store on startup. Serein gives extensions no client internals and no
//! background command channel: every call change is a proposal the user confirms with
//! **Apply**. This port therefore keeps the saved channel, the join-deafened and
//! join-muted choices and the live status readout, and offers them through one native
//! panel.
//!
//! Capabilities used: `app_context` and `voice_state` (status), `channel_directory`
//! and `guild_directory` (voice channel picker), `storage` (settings), `voice_connect`
//! (`JoinVoice`) and `voice_control` (`SetVoice`, `LeaveVoice`).

use serein_extension_sdk::{
	AppAction, AppInvocation, AppOutput, AppSnapshot, ChannelSnapshot, Element,
	GuildDirectorySnapshot, HostEffect, Invocation, Output,
};
use serde::{Deserialize, Serialize};

/// Discord channel types that accept voice participation (voice, stage voice).
const VOICE_KINDS: [u8; 2] = [2, 13];
/// `Element::Select` rejects more options than this.
const MAX_OPTIONS: usize = 32;
/// `Element::Select` rejects longer option labels than this.
const MAX_OPTION_BYTES: usize = 128;
/// Room kept for the ` (channel-id)` suffix inside an option label.
const MAX_OPTION_PREFIX: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
	channel_id: String,
	/// `true` joins the call with its audio muted; the original had this on by default.
	deafened: bool,
	muted: bool,
	/// The panel shows a free-text ID field instead of the loaded-channel picker.
	manual: bool,
	/// Offer the join proposal as soon as the tool opens; this is the closest the host
	/// allows to the original "join on startup", and it still needs Apply.
	propose_open: bool,
}

impl Default for Saved {
	fn default() -> Self {
		Self {
			channel_id: String::new(),
			deafened: true,
			muted: false,
			manual: false,
			propose_open: true,
		}
	}
}

impl Saved {
	/// Drop a saved value that is not a Discord ID, so the panel never proposes it.
	fn normalize(&mut self) {
		if !self.channel_id.is_empty() && !valid_id(&self.channel_id) {
			self.channel_id.clear();
		}
	}
}

/// Discord entity IDs are nonzero decimal `u64` values written as strings.
fn valid_id(value: &str) -> bool {
	!value.is_empty()
		&& value.len() <= 20
		&& value.bytes().all(|byte| byte.is_ascii_digit())
		&& value
			.parse::<u64>()
			.is_ok_and(|parsed| parsed != 0)
}

fn clean(text: &str) -> String {
	text.chars()
		.filter(|character| !character.is_control())
		.collect::<String>()
		.trim()
		.to_string()
}

/// Cut to a byte budget without splitting a character or leaving trailing spaces.
fn truncate(text: &str, max: usize) -> String {
	if text.len() <= max {
		return text.to_string();
	}
	let mut end = max;
	while end > 0 && !text.is_char_boundary(end) {
		end -= 1;
	}
	text[..end].trim_end().to_string()
}

/// `Guild — name (channel-id)`: readable, and the ID stays recoverable from the label.
fn option_label(channel: &ChannelSnapshot, guilds: Option<&GuildDirectorySnapshot>) -> String {
	let guild = match (channel.guild_id.as_deref(), guilds) {
		(Some(id), Some(directory)) => directory
			.items
			.iter()
			.find(|guild| guild.id == id)
			.map(|guild| clean(&guild.name))
			.filter(|name| !name.is_empty()),
		_ => None,
	};
	let name = clean(&channel.name);
	let name = if name.is_empty() {
		format!("Channel {}", channel.id)
	} else {
		name
	};
	let prefix = match guild {
		Some(guild) => format!("{guild} — {name}"),
		None => name,
	};
	let label = format!("{} ({})", truncate(&prefix, MAX_OPTION_PREFIX), channel.id);
	debug_assert!(label.len() <= MAX_OPTION_BYTES, "{label}");
	label
}

fn option_id(label: &str) -> Option<String> {
	let (_, tail) = label.rsplit_once(" (")?;
	let id = tail.strip_suffix(')')?;
	valid_id(id).then(|| id.to_string())
}

fn channel_name(app: Option<&AppSnapshot>, id: &str) -> String {
	app.and_then(|app| app.channels.as_ref())
		.and_then(|directory| directory.items.iter().find(|channel| channel.id == id))
		.map(|channel| clean(&channel.name))
		.filter(|name| !name.is_empty())
		.unwrap_or_else(|| id.to_string())
}

fn channel_options(app: Option<&AppSnapshot>) -> Vec<String> {
	let Some(app) = app else {
		return Vec::new();
	};
	let Some(directory) = app.channels.as_ref() else {
		return Vec::new();
	};
	let mut labels: Vec<String> = directory
		.items
		.iter()
		.filter(|channel| VOICE_KINDS.contains(&channel.kind))
		.map(|channel| option_label(channel, app.guilds.as_ref()))
		.collect();
	labels.sort();
	labels.dedup();
	labels.truncate(MAX_OPTIONS - 1);
	labels
}

/// The picker always offers the saved channel, even when the directory no longer lists it.
/// An empty directory yields no picker at all, so the ID field takes over.
fn channel_select(saved: &Saved, app: Option<&AppSnapshot>) -> Option<Element> {
	let mut options = channel_options(app);
	if options.is_empty() {
		return None;
	}
	let current = options
		.iter()
		.position(|option| option_id(option).as_deref() == Some(saved.channel_id.as_str()));
	let value = match current {
		Some(index) => options[index].clone(),
		None if !saved.channel_id.is_empty() => {
			if options.len() >= MAX_OPTIONS {
				options.pop();
			}
			let label = format!("Saved channel ({})", saved.channel_id);
			options.push(label.clone());
			label
		}
		None => options.first().cloned()?,
	};
	Some(Element::Select {
		id: "channel".into(),
		label: "Voice channel".into(),
		options,
		value,
	})
}

/// Whether the panel shows the free-text ID field instead of the picker.
fn manual_entry(saved: &Saved, app: Option<&AppSnapshot>) -> bool {
	saved.manual || channel_select(saved, app).is_none()
}

fn yes_no(value: bool) -> &'static str {
	if value { "yes" } else { "no" }
}

fn in_call(app: Option<&AppSnapshot>) -> Option<&serein_extension_sdk::VoiceSnapshot> {
	app.and_then(|app| app.voice.as_ref())
		.filter(|voice| voice.channel_id.is_some())
}

fn deafen_button(app: Option<&AppSnapshot>) -> String {
	match in_call(app) {
		Some(voice) if voice.deafened => "Undeafen this call".into(),
		_ => "Deafen this call".into(),
	}
}

fn connected(app: Option<&AppSnapshot>) -> bool {
	app.and_then(|app| app.context.as_ref())
		.is_some_and(|context| context.connected)
}

fn status(app: Option<&AppSnapshot>, saved: &Saved) -> String {
	let mut lines = vec![if connected(app) {
		"Connected to Discord.".to_string()
	} else {
		"Not connected to Discord; joining and call controls need a signed-in session."
			.to_string()
	}];
	match in_call(app) {
		Some(voice) => lines.push(format!(
			"Current call: {}. Microphone muted: {}. Deafened: {}.",
			channel_name(app, voice.channel_id.as_deref().unwrap_or_default()),
			yes_no(voice.muted),
			yes_no(voice.deafened)
		)),
		None if app.and_then(|app| app.voice.as_ref()).is_some() => {
			lines.push("Not in a call.".to_string());
		}
		None => lines.push("The current call could not be read.".to_string()),
	}
	if saved.channel_id.is_empty() {
		lines.push("No channel saved yet: choose one below, then press Join now.".to_string());
	} else {
		lines.push(format!(
			"Saved channel: {}.",
			channel_name(app, &saved.channel_id)
		));
	}
	lines.join("\n")
}

/// Why the picker cannot show every joinable channel, or `None` when it can.
fn channel_hint(app: Option<&AppSnapshot>) -> Option<&'static str> {
	match app.and_then(|app| app.channels.as_ref()) {
		None => Some("The loaded channel list is unavailable, so type the channel ID above."),
		Some(directory) if directory.truncated => Some(
			"The loaded channel list is partial and stops at the host's limit; type the channel ID above if yours is missing.",
		),
		Some(_) => None,
	}
}

const HELP: &str = "The channel and the two audio choices below are remembered. Opening this tool proposes the join, and Apply confirms it: Serein never lets an extension enter a call on its own, so the original Dorion \"join silently at startup\" cannot be reproduced.";

fn panel(saved: &Saved, app: Option<&AppSnapshot>, status: &str, notice: &str) -> Vec<Element> {
	let mut elements = vec![
		Element::Heading {
			text: "AutoJoinVC".into(),
		},
		Element::Text {
			text: status.into(),
		},
		Element::Separator,
	];
	// The ID field is always reachable: without it an empty channel list would leave no
	// way to configure the plugin at all.
	let picker = channel_select(saved, app);
	let manual = manual_entry(saved, app);
	if manual {
		elements.push(Element::TextInput {
			id: "channel-id".into(),
			label: "Voice channel ID".into(),
			value: saved.channel_id.clone(),
		});
	} else if let Some(select) = picker.clone() {
		elements.push(select);
	}
	if let Some(hint) = channel_hint(app) {
		elements.push(Element::Text { text: hint.into() });
	}
	if picker.is_some() {
		elements.push(Element::Checkbox {
			id: "manual-mode".into(),
			label: "Enter a channel ID instead of choosing from the list".into(),
			checked: manual,
		});
	}
	elements.push(Element::Checkbox {
		id: "propose-open".into(),
		label: "Propose joining when I open AutoJoinVC".into(),
		checked: saved.propose_open,
	});
	elements.push(Element::Checkbox {
		id: "join-deafened".into(),
		label: "Join deafened (mute the call's audio)".into(),
		checked: saved.deafened,
	});
	elements.push(Element::Checkbox {
		id: "join-muted".into(),
		label: "Join with the microphone muted".into(),
		checked: saved.muted,
	});
	elements.push(Element::Row {
		children: vec![
			Element::Button {
				id: "join".into(),
				label: "Join now".into(),
			},
			Element::Button {
				id: "leave".into(),
				label: "Leave voice".into(),
			},
		],
	});
	elements.push(Element::Row {
		children: vec![
			Element::Button {
				id: "deafen".into(),
				label: deafen_button(app),
			},
			Element::Button {
				id: "save".into(),
				label: "Save".into(),
			},
		],
	});
	elements.push(Element::Separator);
	elements.push(Element::Text { text: HELP.into() });
	if !notice.is_empty() {
		elements.push(Element::Text {
			text: notice.into(),
		});
	}
	elements
}

/// Read the form values this click carried; absent values keep what is already saved.
fn merge_form(
	input: &Invocation,
	app: Option<&AppSnapshot>,
	saved: &mut Saved,
) -> Option<&'static str> {
	if let Ok(Some(flag)) = input.parse_value::<bool>("manual-mode") {
		saved.manual = flag;
	}
	if let Ok(Some(flag)) = input.parse_value::<bool>("propose-open") {
		saved.propose_open = flag;
	}
	if let Ok(Some(flag)) = input.parse_value::<bool>("join-deafened") {
		saved.deafened = flag;
	}
	if let Ok(Some(flag)) = input.parse_value::<bool>("join-muted") {
		saved.muted = flag;
	}
	let mut problem = None;
	// Read the control the panel actually rendered. The host keeps a panel's field
	// values for the whole session, so trusting a stale hidden field would resurrect
	// an older channel after the user picked a new one.
	if manual_entry(saved, app) {
		if let Some(text) = input.value("channel-id") {
			let text = text.trim();
			if !text.is_empty() {
				if valid_id(text) {
					saved.channel_id = text.to_string();
				} else {
					problem =
						Some("That is not a Discord channel ID: digits only. Nothing was saved.");
				}
			}
		}
	} else if let Some(selected) = input.value("channel") {
		match option_id(selected) {
			Some(id) => saved.channel_id = id,
			None if !selected.trim().is_empty() => {
				problem = Some("That channel entry could not be read; reopen AutoJoinVC.");
			}
			None => {}
		}
	}
	problem
}

/// The join proposal shared by the Join button and by opening the tool.
fn join_proposal(
	saved: &Saved,
	app: Option<&AppSnapshot>,
) -> Result<(HostEffect, String), String> {
	if !connected(app) {
		return Err("Not connected to Discord; join once the session is ready.".into());
	}
	if saved.channel_id.is_empty() {
		return Err("Choose a voice channel first, then press Join now.".into());
	}
	if let Some(voice) =
		in_call(app).filter(|voice| voice.channel_id.as_deref() == Some(saved.channel_id.as_str()))
	{
		if saved.deafened && !voice.deafened {
			return Ok((
				HostEffect::SetVoice {
					muted: voice.muted,
					deafened: true,
				},
				"Already connected to that channel; proposing to deafen it.".into(),
			));
		}
		return Err("Already connected to that channel.".into());
	}
	let notice = format!(
		"Proposed: join {} (deafened: {}, microphone muted: {}). Press Apply to continue.",
		channel_name(app, &saved.channel_id),
		yes_no(saved.deafened),
		yes_no(saved.muted)
	);
	Ok((
		HostEffect::AppAction {
			action: AppAction::JoinVoice {
				channel_id: saved.channel_id.clone(),
				ring: false,
				muted: saved.muted,
				deafened: saved.deafened,
			},
		},
		notice,
	))
}

fn propose_join(
	saved: &Saved,
	app: Option<&AppSnapshot>,
	problem: Option<&'static str>,
	notice: &mut String,
	effects: &mut Vec<HostEffect>,
) {
	if let Some(message) = problem {
		*notice = message.to_string();
		return;
	}
	match join_proposal(saved, app) {
		Ok((effect, message)) => {
			*notice = message;
			effects.push(effect);
		}
		Err(message) => *notice = message,
	}
}

fn handle(input: AppInvocation) -> AppOutput {
	let mut saved = input
		.invocation
		.storage_json::<Saved>()
		.ok()
		.flatten()
		.unwrap_or_default();
	saved.normalize();
	let app = input.app.as_ref();
	let mut output = Output::default();
	if input.invocation.action == "activate" {
		// Activation restores and repairs storage; this host rejects proposals from it.
		if output.set_storage_json(&saved).is_err() {
			return AppOutput::default();
		}
		return AppOutput {
			output,
			effects: Vec::new(),
		};
	}
	let problem = merge_form(&input.invocation, app, &mut saved);
	let mut notice = String::new();
	let mut effects: Vec<HostEffect> = Vec::new();
	match input.invocation.action.as_str() {
		// Opening the tool proposes the join straight away: that is the closest this
		// host gets to the original "join when the client starts".
		"open" => {
			if saved.propose_open {
				propose_join(&saved, app, problem, &mut notice, &mut effects);
			}
		}
		"join" => propose_join(&saved, app, problem, &mut notice, &mut effects),
		"leave" => match in_call(app) {
			Some(voice) => {
				notice = format!(
					"Proposed: leave {}. Press Apply to continue.",
					channel_name(app, voice.channel_id.as_deref().unwrap_or_default())
				);
				effects.push(HostEffect::LeaveVoice);
			}
			None => notice = "There is no active call to leave.".into(),
		},
		"deafen" => match in_call(app) {
			Some(voice) => {
				let deafened = !voice.deafened;
				notice = format!(
					"Proposed: {}. Press Apply to continue.",
					if deafened {
						"deafen this call"
					} else {
						"undeafen this call"
					}
				);
				effects.push(HostEffect::SetVoice {
					muted: voice.muted,
					deafened,
				});
			}
			None => notice = "Join a call first; deafen applies to the current call only.".into(),
		},
		"save" => {
			notice = match problem {
				Some(message) => message.to_string(),
				None if saved.channel_id.is_empty() => {
					"Saved the options. Choose a voice channel to remember one.".into()
				}
				None => format!("Saved {}.", channel_name(app, &saved.channel_id)),
			};
		}
		_ => return AppOutput::default(),
	}
	if output.set_storage_json(&saved).is_err() {
		notice = "Could not save these options; they will not survive a restart.".into();
	}
	let status = status(app, &saved);
	output.panel = panel(&saved, app, &status, &notice);
	AppOutput { output, effects }
}

serein_extension_sdk::export!(handle);

#[cfg(test)]
mod tests {
	use super::*;
	use serein_extension_sdk::{
		AppContextSnapshot, ChannelDirectorySnapshot, GuildDirectorySnapshot, GuildSnapshot,
		VoiceSnapshot, dispatch_typed, serde_json,
	};

	const GUILD: &str = "10";
	const CHANNEL: &str = "20";
	const OTHER: &str = "21";

	fn channel(id: &str, name: &str, kind: u8) -> ChannelSnapshot {
		ChannelSnapshot {
			id: id.into(),
			guild_id: Some(GUILD.into()),
			name: name.into(),
			kind,
		}
	}

	fn snapshot(voice: Option<(&str, bool, bool)>) -> AppSnapshot {
		AppSnapshot {
			context: Some(AppContextSnapshot {
				connected: true,
				user: None,
				channel: None,
			}),
			guilds: Some(GuildDirectorySnapshot {
				items: vec![GuildSnapshot {
					id: GUILD.into(),
					name: "Home".into(),
					icon: None,
				}],
				truncated: false,
			}),
			channels: Some(ChannelDirectorySnapshot {
				items: vec![
					channel(CHANNEL, "general", 2),
					channel(OTHER, "stage", 13),
					channel("30", "chat", 0),
				],
				truncated: false,
			}),
			voice: voice.map(|(channel, muted, deafened)| VoiceSnapshot {
				channel_id: Some(channel.into()),
				phase: "connected".into(),
				muted,
				deafened,
				camera: false,
				streaming: false,
				participants: Vec::new(),
			}),
			..Default::default()
		}
	}

	fn run(action: &str, app: Option<AppSnapshot>) -> AppOutput {
		handle(AppInvocation {
			invocation: Invocation {
				action: action.into(),
				..Default::default()
			},
			app,
			..Default::default()
		})
	}

	fn picker_value(name: &str, id: &str) -> String {
		format!("Home — {name} ({id})")
	}

	fn select(options_output: &AppOutput) -> Vec<String> {
		options_output
			.output
			.panel
			.iter()
			.find_map(|element| match element {
				Element::Select { options, .. } => Some(options.clone()),
				_ => None,
			})
			.expect("voice channel picker")
	}

	#[test]
	fn open_is_passive_and_offers_only_voice_channels() {
		let output = run("open", Some(snapshot(None)));
		assert!(output.effects.is_empty());
		let options = select(&output);
		assert_eq!(options.len(), 2);
		assert!(options.iter().all(|option| option_id(option).is_some()));
		assert!(!options.iter().any(|option| option.contains("chat")));
	}

	#[test]
	fn join_proposes_the_saved_channel_with_the_stored_audio_choices() {
		let mut invocation = Invocation {
			action: "join".into(),
			..Default::default()
		};
		invocation
			.values
			.insert("channel".into(), picker_value("general", CHANNEL));
		invocation
			.values
			.insert("join-deafened".into(), "true".into());
		invocation
			.values
			.insert("join-muted".into(), "false".into());
		let output = handle(AppInvocation {
			invocation,
			app: Some(snapshot(None)),
			..Default::default()
		});
		assert!(matches!(
			output.effects.as_slice(),
			[HostEffect::AppAction {
				action: AppAction::JoinVoice {
					channel_id,
					ring: false,
					muted: false,
					deafened: true,
				}
			}] if channel_id == CHANNEL
		));
		let saved: Saved =
			serde_json::from_str(output.output.storage.as_deref().expect("saved")).unwrap();
		assert_eq!(saved.channel_id, CHANNEL);
		assert!(saved.deafened);
	}

	#[test]
	fn join_without_a_channel_proposes_nothing() {
		let output = run("join", Some(snapshot(None)));
		assert!(output.effects.is_empty());
		assert!(output.output.panel.iter().any(|element| matches!(
			element,
			Element::Text { text } if text.contains("Choose a voice channel")
		)));
	}

	#[test]
	fn join_without_a_session_proposes_nothing() {
		let mut invocation = Invocation {
			action: "join".into(),
			storage: Some(format!(r#"{{"channel_id":"{CHANNEL}"}}"#)),
			..Default::default()
		};
		invocation
			.values
			.insert("channel".into(), picker_value("general", CHANNEL));
		let output = handle(AppInvocation {
			invocation,
			app: Some(AppSnapshot::default()),
			..Default::default()
		});
		assert!(output.effects.is_empty());
	}

	#[test]
	fn joining_the_current_channel_only_deafens_it() {
		let mut invocation = Invocation {
			action: "join".into(),
			storage: Some(format!(
				r#"{{"channel_id":"{CHANNEL}","deafened":true}}"#
			)),
			..Default::default()
		};
		invocation
			.values
			.insert("channel".into(), picker_value("general", CHANNEL));
		let output = handle(AppInvocation {
			invocation,
			app: Some(snapshot(Some((CHANNEL, false, false)))),
			..Default::default()
		});
		assert!(matches!(
			output.effects.as_slice(),
			[HostEffect::SetVoice {
				muted: false,
				deafened: true
			}]
		));
	}

	#[test]
	fn leave_and_deafen_need_a_current_call() {
		assert!(run("leave", Some(snapshot(None))).effects.is_empty());
		assert!(run("deafen", Some(snapshot(None))).effects.is_empty());
		assert!(matches!(
			run("leave", Some(snapshot(Some((CHANNEL, false, false))))).effects.as_slice(),
			[HostEffect::LeaveVoice]
		));
		assert!(matches!(
			run("deafen", Some(snapshot(Some((CHANNEL, false, true))))).effects.as_slice(),
			[HostEffect::SetVoice {
				muted: false,
				deafened: false
			}]
		));
	}

	#[test]
	fn manual_entry_replaces_the_picker_and_rejects_bad_ids() {
		let mut invocation = Invocation {
			action: "save".into(),
			..Default::default()
		};
		invocation
			.values
			.insert("manual-mode".into(), "true".into());
		invocation
			.values
			.insert("channel-id".into(), "not-an-id".into());
		let output = handle(AppInvocation {
			invocation,
			app: Some(snapshot(None)),
			..Default::default()
		});
		let saved: Saved = serde_json::from_str(output.output.storage.as_deref().unwrap()).unwrap();
		assert!(saved.channel_id.is_empty(), "invalid id must not be stored");
		assert!(saved.manual);
		assert!(output.output.panel.iter().any(|element| matches!(
			element,
			Element::Text { text } if text.contains("not a Discord channel ID")
		)));
	}

	#[test]
	fn manual_entry_accepts_a_real_id_without_the_picker() {
		let mut invocation = Invocation {
			action: "join".into(),
			storage: Some(r#"{"manual":true}"#.into()),
			..Default::default()
		};
		invocation
			.values
			.insert("manual-mode".into(), "true".into());
		invocation.values.insert("channel-id".into(), OTHER.into());
		let output = handle(AppInvocation {
			invocation,
			app: Some(snapshot(None)),
			..Default::default()
		});
		assert!(matches!(
			output.effects.as_slice(),
			[HostEffect::AppAction {
				action: AppAction::JoinVoice { channel_id, .. }
			}] if channel_id == OTHER
		));
		assert!(
			output
				.output
				.panel
				.iter()
				.any(|element| matches!(element, Element::TextInput { .. }))
		);
	}

	#[test]
	fn activation_repairs_corrupt_storage_and_never_proposes_a_call() {
		let output = handle(AppInvocation {
			invocation: Invocation {
				action: "activate".into(),
				storage: Some(r#"{"channel_id":"nope"}"#.into()),
				..Default::default()
			},
			app: Some(snapshot(None)),
			..Default::default()
		});
		assert!(output.effects.is_empty());
		let saved: Saved = serde_json::from_str(output.output.storage.as_deref().unwrap()).unwrap();
		assert!(saved.channel_id.is_empty());
		assert!(saved.deafened, "the original default is join-deafened");
	}

	#[test]
	fn the_saved_channel_stays_selectable_when_the_directory_forgets_it() {
		let saved = Saved {
			channel_id: "999".into(),
			..Default::default()
		};
		match channel_select(&saved, Some(&snapshot(None))).expect("picker") {
			Element::Select { options, value, .. } => {
				assert!(options.contains(&value));
				assert_eq!(option_id(&value).as_deref(), Some("999"));
			}
			_ => panic!("expected a select"),
		}
	}

	#[test]
	fn a_known_saved_channel_is_selected_in_the_picker() {
		let saved = Saved {
			channel_id: CHANNEL.into(),
			..Default::default()
		};
		match channel_select(&saved, Some(&snapshot(None))).expect("picker") {
			Element::Select { value, .. } => {
				assert_eq!(option_id(&value).as_deref(), Some(CHANNEL));
			}
			_ => panic!("expected a select"),
		}
	}

	#[test]
	fn long_names_keep_the_option_label_within_the_host_limit() {
		let long = "ч".repeat(200);
		let label = option_label(&channel(CHANNEL, &long, 2), None);
		assert!(label.len() <= MAX_OPTION_BYTES, "{label}");
		assert!(label.chars().all(|character| !character.is_control()));
		assert_eq!(option_id(&label).as_deref(), Some(CHANNEL));
	}

	#[test]
	fn a_saved_channel_without_a_directory_uses_the_id_field() {
		let saved = Saved {
			channel_id: CHANNEL.into(),
			..Default::default()
		};
		assert!(channel_select(&saved, None).is_none());
		assert!(manual_entry(&saved, None));
	}

	#[test]
	fn the_wire_round_trip_matches_the_abi() {
		let bytes = dispatch_typed(
			br#"{"action":"open","app":{"context":{"connected":true}}}"#,
			handle,
		)
		.unwrap();
		let output: AppOutput = serde_json::from_slice(&bytes).unwrap();
		assert!(output.output.panel.iter().any(|element| matches!(
			element,
			Element::Text { text } if text.contains("Connected to Discord.")
		)));
	}

	#[test]
	fn opening_the_tool_proposes_the_saved_channel() {
		let output = handle(AppInvocation {
			invocation: Invocation {
				action: "open".into(),
				storage: Some(format!(r#"{{"channel_id":"{CHANNEL}","deafened":true}}"#)),
				..Default::default()
			},
			app: Some(snapshot(None)),
			..Default::default()
		});
		assert!(matches!(
			output.effects.as_slice(),
			[HostEffect::AppAction {
				action: AppAction::JoinVoice {
					channel_id,
					deafened: true,
					..
				}
			}] if channel_id == CHANNEL
		));
	}

	#[test]
	fn opening_the_tool_stays_passive_when_the_option_is_off() {
		let output = handle(AppInvocation {
			invocation: Invocation {
				action: "open".into(),
				storage: Some(format!(
					r#"{{"channel_id":"{CHANNEL}","propose_open":false}}"#
				)),
				..Default::default()
			},
			app: Some(snapshot(None)),
			..Default::default()
		});
		assert!(output.effects.is_empty());
	}

	/// The host's directory can be empty or truncated; the panel must stay configurable.
	#[test]
	fn an_empty_channel_list_still_offers_the_id_field() {
		let output = handle(AppInvocation {
			invocation: Invocation {
				action: "open".into(),
				..Default::default()
			},
			app: Some(AppSnapshot {
				context: Some(AppContextSnapshot {
					connected: true,
					user: None,
					channel: None,
				}),
				..Default::default()
			}),
			..Default::default()
		});
		assert!(output.output.panel.iter().any(
			|element| matches!(element, Element::TextInput { id, .. } if id == "channel-id")
		));
		assert!(
			!output
				.output
				.panel
				.iter()
				.any(|element| matches!(element, Element::Select { .. }))
		);
		assert!(!output.output.panel.iter().any(
			|element| matches!(element, Element::Checkbox { id, .. } if id == "manual-mode")
		));
	}

	#[test]
	fn a_typed_id_is_used_before_any_channel_is_known() {
		let mut invocation = Invocation {
			action: "join".into(),
			..Default::default()
		};
		invocation.values.insert("channel-id".into(), OTHER.into());
		let output = handle(AppInvocation {
			invocation,
			app: Some(AppSnapshot {
				context: Some(AppContextSnapshot {
					connected: true,
					user: None,
					channel: None,
				}),
				..Default::default()
			}),
			..Default::default()
		});
		assert!(matches!(
			output.effects.as_slice(),
			[HostEffect::AppAction {
				action: AppAction::JoinVoice { channel_id, .. }
			}] if channel_id == OTHER
		));
	}

	/// The host keeps field values for the whole panel session, so a hidden text field
	/// must not override the picker the user actually sees.
	#[test]
	fn a_stale_id_field_does_not_override_the_picker() {
		let mut invocation = Invocation {
			action: "save".into(),
			storage: Some(format!(r#"{{"channel_id":"{OTHER}"}}"#)),
			..Default::default()
		};
		invocation
			.values
			.insert("channel".into(), picker_value("general", CHANNEL));
		invocation
			.values
			.insert("channel-id".into(), "999".into());
		let output = handle(AppInvocation {
			invocation,
			app: Some(snapshot(None)),
			..Default::default()
		});
		let saved: Saved = serde_json::from_str(output.output.storage.as_deref().unwrap()).unwrap();
		assert_eq!(saved.channel_id, CHANNEL);
	}
}
