# Desktop workflow update — 2026-09-07

This update keeps the native Tauri Windows client, existing icon and light/dark
themes. It does not publish a website or modify the main branch.

## Using the updated client

1. **Hosts and passwords**: enter the login password while adding/editing a host.
   The host definition goes into the existing host store; the password goes into
   the Windows credential manager under the SSH user/address/port. Empty password
   fields keep the existing credential. Reconnecting uses saved credentials
   without opening the authentication dialog. Authentication failures still show
   a dialog so the user can correct them. The connection dialog remembers newly
   entered passwords by default; this can be unchecked.
2. **Connections**: the independent Sessions navigation item is removed. Active
   SSH transports remain shared by terminal, SFTP and forwarding. Each connected
   host's terminal stays mounted when switching pages or hosts. Disconnect is in
   the terminal toolbar. The plus button opens another independent terminal on
   the current host, reusing its authenticated SSH transport. Each terminal has
   its own shell, working directory, output and close button. Closing the last
   terminal leaves SSH, SFTP and port forwarding connected; “断开主机” closes
   all of them. Host creation remains in the sidebar and top-right button.
   Neither terminal tabs nor live connections are restored after process exit.
3. **Information**: opening the information page immediately samples the selected
   host, then samples every 10 seconds. Switching targets discards old responses
   and sample history; requests for the same target do not overlap. Leaving the
   page or hiding the window stops periodic collection. CPU bars use actual
   samples, not decorative values. Remote Linux CPU utilization comes from two
   /proc/stat snapshots; CPU load averages remain separate. NVIDIA devices use
   nvidia-smi, with MiB converted to bytes. Missing data is shown as unavailable.
   The route-node selector describes routing; the **query target selector** is
   what changes the monitored machine. A remote target must first be connected.
4. **Forwarding**: the diagram shows the client, every SSH jump and final SSH
   endpoint with name, address/port and user. The remote service endpoint is
   separately labeled, because it is not another SSH login. Remote (-R)
   forwarding shows reverse data flow. Active forward routes are snapshots of
   the actual connected transport, not subsequently edited host definitions.
   Port forwards are runtime resources and stop on disconnection/exit.
5. **Run history**: explicitly executed commands are now available in the
   History page's command bar. Completed success/failure records append to
   desktop-history.jsonl next to hosts.toml and are synchronized to disk.
   History survives restarts and has no automatic 200-record retention limit.
   The backend returns 50 records per page, newest first, using bounded memory.
   Clearing archives the exact log as a timestamped .bak rather than deleting it.
   Interactive terminal keystrokes are deliberately not recorded: password
   prompts cannot reliably be distinguished from ordinary keystrokes.
   Commands and output previews may contain sensitive data; users must not put
   passwords/tokens into commands they choose to record.
6. **Kaduox AI**: third-party OpenAI-compatible HTTP APIs only; the offline
   canned-answer branch is removed. Custom provider name, endpoint and model
   are saved without secrets. “获取模型列表” requests the provider's /models
   endpoint; selection and manual model entry are both supported. Providers
   lacking this endpoint can still be used by entering a model ID manually.
   API keys are scoped by provider ID AND normalized endpoint; switching vendors
   clears any unsaved key. Prior globally scoped keys are not automatically
   reused against an unknown vendor; save each provider's key once after upgrade.
   Non-loopback HTTP and redirects are refused. Responses are capped at 2 MiB.
    Host context is sent only when the corresponding checkbox is selected.
    When a connected target host is selected, the AI may request command
    execution through the `execute_command` tool under a Codex-style dual
    permission mode: "请求批准" (default) auto-runs read-only commands and asks
    for manual approval on modify/delete; "全部权限" (opt-in with a confirmation
    dialog) also auto-runs modify commands. Delete-class commands always require
    manual approval, and dangerous commands (fork bombs, `rm -rf /`, raw disk
    writes) are refused outright. Each AI round produces at most one batched
    approval prompt; intermediate rounds collapse to compact command cards and
    only the final consolidated answer renders as Markdown. Every AI-run
    command lands in run history with the `AI` source badge and in the SQLite
    command audit. Conversations persist in `ai-chat.db` next to the host
    store; provider profiles can be deleted, which also removes their saved
    API key.
7. **Search**: search has visible results on any page, matches name/address/user/
   tags/groups, supports arrow keys, Enter and Escape, and navigates to the
   selected host. Windows/Linux show Ctrl+K and Ctrl+Enter; macOS shows Command.
8. **Jump chains**: save each jump host with its own password or identity, then
   add up to five jumps in the order they must be reached. The final target is
   additional (five jumps plus one target). Each jump is individually
   authenticated using its own saved credential. A host's own chain is not
   implicitly nested into another chain. Imported inline/OpenSSH hop definitions
   are preserved when an existing chain is edited.
9. **Host purpose** (2026-09-08): adding/editing a host now offers “目标机器”,
   “中转机器”, and “两者兼用”. New GUI records default to target; existing records
   without a purpose load as both, without rewriting them automatically. The
   sidebar displays this choice. SSH jump candidates must be jump/both; the
   forwarding page's final SSH target selector accepts target/both. This is a
   routing aid, not access control: a jump host can still be opened directly for
   maintenance. Marking a referenced jump as target-only is rejected until its
   chain references are removed. Changing purpose never creates a route by itself.

   For this illustrative route: mark 192.0.2.10 as “中转机器” and
   198.51.100.20 as “目标机器”; edit the latter, select the former as jump 1,
   and save the target. Then connect to the target: client → jump → target.
   The client authenticates each node with that node's saved credential.
   Existing user records/routes have not been changed by the implementation.

   Purpose is an optional `role = "target" | "jump" | "both"` host-store field.
   Updated CLI/MCP builds understand it; MCP host listing exposes it. Older
   binaries that reject unknown fields must be upgraded before sharing a host
   database containing explicit roles.

## Validation — multi-terminal and host-purpose update (2026-09-08)

- 30 frontend tests and TypeScript/Vite production build passed.
- 186 SSH core, 16 host-store, 6 MCP and 25 desktop unit tests passed.
- Rust formatting and clippy with warnings denied passed for the changed crates
  and desktop backend; git diff whitespace checks passed.
- A real SSH check explicitly ran two independent PTYs on one authenticated
  transport. Their working directories and shell variables stayed independent;
  closing one kept the other usable and released the closed shell's remote PID.
  This exposed and fixed cancellation leaving the remote shell alive: the core
  now sends SSH channel CLOSE when the interactive-shell future is dropped.
- The opt-in test uses KADUOX_LIVE_TERMINAL_ALIAS and that endpoint's existing
  system-stored password. It performs shell-local variable/directory operations,
  disables shell history, and checks only its own shell PID with kill -0.
  It does not save host definitions, routes, credentials or remote files.
- Light/dark browser previews were inspected with multiple tabs and host-purpose
  editing. Preview data remains a UI fixture, not evidence of real connectivity.
- Earlier five-hop/forwarding integration checks below were not rerun for this
  update. Clean-machine installer verification remains manual.
- The additional Windows Rust 1.85 check is blocked by the existing pageant
  0.2.2 dependency's let-chain syntax (requires Rust 1.88). No dependency pins
  were changed for this update. Successful checks/builds here use Rust 1.98.0;
  the desktop manifest already declares Rust 1.88 as its minimum.

```powershell
$env:KADUOX_LIVE_TERMINAL_ALIAS = '<saved-host-alias>'
cargo test --manifest-path apps/kaduox-desktop/src-tauri/Cargo.toml --locked two_real_terminals -- --ignored
```

## Earlier validation

- Frontend TypeScript/Vite production build and 19 frontend tests.
- Desktop Rust fmt, check, clippy with warnings denied, and 25 unit tests.
- An additional ignored-by-default integration test was explicitly run from
   Windows against six isolated OpenSSH instances in WSL: five real jump
   transports and a final target; command output matched five-hops-ok.
- Windows credential save, fresh lookup and deletion were tested with a unique
   synthetic credential, not an existing user's credential.
- AI models/chat were exercised through real localhost HTTP fixture servers,
   including bearer authentication, model deduplication, offline rejection,
   and suppression of reflected secrets in HTTP errors. No paid vendor API
   or user's production API key was used for these tests.
- The read-only Linux metrics script was run in WSL and returned actual CPU,
   NVIDIA GPU, memory, disk and network counters.
- Browser preview is for layout inspection only; preview fixtures are not
   evidence of real SSH or vendor connectivity. Windows 10/11 clean-machine
   installation and external vendor-specific compatibility remain manual checks.

## Reproducing the five-hop check

Run scripts/ci/desktop-five-hop-fixture.sh as root in an isolated WSL instance.
Pass a disposable parent directory; it creates its own uniquely named child
and uses ports 41251–41256 (refuses occupied ports). It does not change the system
sshd configuration or users' authorized_keys. It runs for four minutes and
stops its daemons on exit. In another terminal, set KADUOX_FIXTURE_KEY to its
generated client key and KADUOX_FIXTURE_HOST to the reported WSL address, then:

```powershell
cargo test --manifest-path apps/kaduox-desktop/src-tauri/Cargo.toml --locked five_real_openssh -- --ignored
```

The fixture uses insecure host-key checking only for these disposable local
servers. Never use that policy for production servers.
