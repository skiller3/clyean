# Sign in to providers

Sign in to model providers and MCP servers through the User Assistant, with `/login` or `/mcp` as in any harness session.  Every sign-in is stored in the User Assistant's login store, and the other agents receive copies of the ones they need.  This guide covers sign-ins that call back to your browser, which need a relay into the sandbox, and the fallbacks when that is not possible.

## Sign in with a browser callback

Several providers finish their sign-in by sending your browser to a callback server that the harness runs on a fixed loopback port, for example OpenRouter on port 54549.  That server runs inside the User Assistant's container, so Clyean relays the port for it:

1. Type `/login` (or `/mcp`, `/model`, or `/clyean sign-in <agent> <server>`) and choose the provider.  Clyean also watches for sign-ins while a session starts, as first-run setup does.
2. When the harness starts its callback server, Clyean notices the new listener, confirms that it is a sign-in callback, and asks for a relay on the same port:

   ```text
   The User Assistant's sign-in to openrouter.ai needs a relay
     TCP  127.0.0.1:54549 and [::1]:54549 on this computer  ->  port 54549 in the User Assistant's container

   > Allow permanently for this project
     Allow permanently for this session
     Allow once
     Deny
   ```

   Choose "Allow permanently for this project" to sign in to the same provider later without a prompt.
3. Once the relay opens, Clyean opens `http://localhost:54549/launch` in your browser, which forwards to the provider's sign-in page.  Sign in there; the provider sends your browser back to the callback, which reaches the harness through the relay.
4. The relay closes two seconds after the callback server stops listening.

The login dialog always shows the sign-in address too, so you can open it yourself when no browser opens.

## Fall back to pasting the address

When the relay cannot open (you denied it, the port is busy, or relaying is off), the sign-in still works: after you sign in, your browser ends on a page that cannot load.  Copy the address from the browser's address bar and paste it into the login dialog, which reads the code from it.  Clyean's warning in the transcript names this fallback.

Other sign-ins need no relay at all:

- **API keys.**  Enter the key in the login dialog, or export the provider's variable (for example `ANTHROPIC_API_KEY`) on the host before starting `clyean`; see [environment variables](../reference/environment-variables.md#passed-into-the-user-assistants-container).
- **Device codes.**  Providers that show a code to enter on a web page work as they do anywhere else.

## Where the browser opens

| Host | Opened with |
| --- | --- |
| Linux | `xdg-open` |
| Linux on WSL | `wslview` when installed, otherwise Windows' `explorer.exe` |
| macOS | `open` |
| Native Windows | `explorer.exe`, which uses your default browser |

A missing or failing opener is not an error; open the address from the login dialog yourself.  To stop Clyean from opening sign-in pages at all, set `"openSignInPages": false` in the [relay file](../reference/relay-allowlist.md).

## Windows and WSL

On native Windows and macOS, relays listen on the host itself, so your browser reaches `localhost` directly.

Inside WSL, relays listen in the Linux distribution:

- In mirrored networking mode, Windows reaches them as `localhost`.
- In NAT mode, Windows reaches TCP relays on loopback through WSL's localhost forwarding.

If the callback page does not load in the Windows browser, use the paste fallback.

## Troubleshoot

| Symptom | Cause and fix |
| --- | --- |
| No prompt appears, and the browser page fails to load | The sign-in's callback was not detected.  Paste the address the browser ends on into the login dialog. |
| `relay_port_unavailable` | Another program, perhaps another `clyean` process signing in to the same provider, holds the callback port.  Finish or cancel that sign-in, or paste the address. |
| The prompt offers only "Allow once" and "Deny" | The relay file does not parse, so nothing can be allowed permanently.  Fix `relays.json`; the prompt says what is wrong with it. |
| A sub-agent reports that it has no credential | Sign in through the User Assistant.  The agent receives a copy the next time it starts. |
