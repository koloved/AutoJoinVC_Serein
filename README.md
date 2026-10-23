# AutoJoinVC for Serein

A port of [koloved/AutoJoinVC](https://github.com/koloved/AutoJoinVC) — the
Dorion/Shelter plugin that puts you in a voice channel on startup — to the
[Serein](https://github.com/ViceVerse-cz/Serein) extension sandbox.

Serein extensions are sandboxed WebAssembly with no client internals and no
background command channel, so this is a rewrite rather than a copy: the same
job, expressed through Serein's native tool panel and its confirmation flow.

## What it does

- Remembers one voice channel and the audio choices to use when joining it.
- Lists the voice and stage channels of the servers you have loaded, with the
  server name, so you pick a channel by name instead of pasting an ID.
  Channels outside the loaded list can still be entered by ID.
- **Join now** proposes joining that channel, deafened and/or with the
  microphone muted, depending on the saved options.
- **Leave voice** proposes leaving the current call.
- **Deafen this call / Undeafen this call** proposes toggling deafen on the call
  you are already in.
- Shows the live state: connection, the current call, and what is saved.

## What changed from the Dorion version

Serein is stricter than the client Mods that Dorion ships, and one difference is
worth knowing before you install:

| Dorion (Shelter) | Serein |
| --- | --- |
| Joined the channel silently on startup by dispatching `VOICE_CHANNEL_SELECT`. | **Cannot join silently.** A host rejects every `effects` proposal from an `activation`, `app_event`, `tick` or `message_event` handler; joining a call can transmit audio, so Serein always shows the proposal and waits for **Apply**. |
| Retried up to six times, three seconds apart. | Apply re-checks that the call, account and channel are still current, and the panel re-reads the live call state each time you press a button. A stale proposal is rejected instead of following a later change. |
| Pasted Guild ID and Voice Channel ID. | A picker built from the loaded channel directory; the guild is resolved from the channel, so the guild ID is no longer needed. Manual ID entry is still there for channels the client has not loaded. |
| Toasts via `s.ui.showToast`. | The panel's own status line; Serein has no unattended toast for a passive plugin. |
| Joined, then self-deafened after the connection settled. | `JoinVoice` takes `deafened` and `muted` directly, so the call starts deafened instead of being muted afterwards. A separate button toggles deafen on the current call. |

In practice: open the tool, press **Join now**, press **Apply**. That second
click is the host's requirement, not a limitation this plugin can work around.

## Requirements

- Serein with extension support.
- Rust **1.85 or newer** and the `wasm32-unknown-unknown` target, only to build.
- Python 3, only to repackage.

## Install

1. In Serein open **Settings → Extensions**.
2. Import `packages/auto-join-vc.serein-extension`.
3. Review the capabilities, then enable the plugin.
4. Open the **AutoJoinVC** card and choose **Open tool**.
5. Pick a voice channel, set *Join deafened* / *Join with the microphone muted*,
   and press **Join now**. Confirm the proposal with **Apply**.

Nothing is stored about a guild, a message or a call; the plugin keeps only the
channel ID and two checkboxes in its own storage slot.

## Permissions

| Capability | Why it is needed |
| --- | --- |
| `app_context` | Report whether the session is connected, so **Join now** can refuse early. |
| `voice_state` | Read the current call, its channel, and the mute/deafen state. |
| `channel_directory` | Build the voice channel picker. |
| `guild_directory` | Label each channel with its server name. |
| `voice_connect` | Propose `join_voice` with the chosen mute/deafen flags. |
| `voice_control` | Propose `leave_voice` and `set_voice`. |
| `storage` | Remember the channel and the two audio choices across restarts. |

The plugin asks for no message, member, presence or moderation data, cannot call
Discord's REST API, and cannot start a call or change anything without **Apply**.

## Build from source

```sh
rustup target add wasm32-unknown-unknown
cargo test --workspace
cargo build --release --target wasm32-unknown-unknown -p auto-join-vc
python pack.py auto-join-vc/manifest.json \
    target/wasm32-unknown-unknown/release/auto_join_vc.wasm \
    packages/auto-join-vc.serein-extension
```

`cargo test` runs the handler through the same JSON path the host uses, and
`cargo clippy --workspace --all-targets -- -D warnings` is clean.

## Verify against a real host

`tools/host_check.rs` is an example for a Serein checkout. It validates the
manifest with the host's own rules and then runs the compiled module through the
real Wasm sandbox, checking the panel, the join/leave proposals and activation:

```sh
cp tools/host_check.rs <serein>/crates/extensions/examples/auto_join_vc_check.rs
cd <serein>
cargo run --locked -p extensions --example auto_join_vc_check -- \
    <this repo>/auto-join-vc/manifest.json \
    <this repo>/target/wasm32-unknown-unknown/release/auto_join_vc.wasm
```

Expected output ends with `Host sandbox accepted the package.` The example is a
development check; it is not part of the packaged extension.

## Layout

| Path | Contents |
| --- | --- |
| `auto-join-vc/` | Manifest and handler source |
| `auto-join-vc/src/lib.rs` | The whole plugin |
| `packages/auto-join-vc.serein-extension` | Installable package built from the source above |
| `sdk/` | Copy of the Serein extension SDK, so the repository builds on its own |
| `pack.py` | Serein's packer: manifest plus module into one JSON package |
| `tools/host_check.rs` | Offline host-sandbox check (see above) |

The SDK in `sdk/` is the work of the Serein contributors and keeps its own
`MIT OR Apache-2.0` licence (`sdk/LICENSE-MIT`, `sdk/LICENSE-APACHE`). Everything
else here is MIT; see `LICENSE`.

## Русский

Порт плагина [AutoJoinVC](https://github.com/koloved/AutoJoinVC) с Dorion/Shelter
на расширения Serein.

Главное отличие: **тихо войти в канал при старте Serein не позволяет** —
расширение не может отправить в звонок команду без подтверждения. Поэтому
плагин запоминает канал и настройки звука, показывает их в панели и предлагает
вход по кнопке **Join now**; вы подтверждаете его кнопкой **Apply**. Канал
выбирается по названию из загруженных серверов, но можно ввести и ID вручную.

Установка: **Settings → Extensions → Import** →
`packages/auto-join-vc.serein-extension`, проверить разрешения, включить, открыть
**AutoJoinVC → Open tool**.

## License

MIT — see [LICENSE](LICENSE).
