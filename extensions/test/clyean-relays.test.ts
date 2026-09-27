// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

import { afterEach, beforeEach, expect, test } from "bun:test";
import {
	createFakeContext,
	createFakePi,
	environmentSandbox,
	importFresh,
	startStubOrchestrator,
	type StubOrchestrator,
	type StubReply,
	waitFor,
} from "./harness";

const MODULE = "../clyean-orchestration.ts";

const sandbox = environmentSandbox();
let server: StubOrchestrator | undefined;

beforeEach(() => sandbox.reset());

afterEach(async () => {
	if (server) {
		await server.close();
		server = undefined;
	}
	sandbox.restore();
});

const PROCESS_FACTS = {
	requester: { kind: "process", pid: 4242, command: "python3 -m http.server 8000" },
	relay: { protocol: "tcp", host_port: 3000, guest_port: 8000, host_addresses: ["127.0.0.1", "::1"] },
	server: null,
	reason: "Preview the page",
	session_available: true,
	permanent_available: true,
	file_problem: null,
	warning: null,
};

test("prompts name the requester, the relay, and only the choices that can be recorded", async () => {
	const { renderApprovalPrompt, APPROVAL_CHOICES } = await importFresh(MODULE);
	const process = renderApprovalPrompt(PROCESS_FACTS);
	expect(process.lines[0]).toBe("Relay request from a process in the User Assistant's container");
	expect(process.lines).toContain("  Process 4242: python3 -m http.server 8000   (as it reports itself)");
	expect(process.lines).toContain("  TCP  127.0.0.1:3000 and [::1]:3000 on this computer  ->  port 8000 in the User Assistant's container");
	expect(process.lines).toContain('  Reason given: "Preview the page"');
	expect(process.options).toEqual([APPROVAL_CHOICES.project, APPROVAL_CHOICES.session, APPROVAL_CHOICES.once, APPROVAL_CHOICES.deny]);

	const server = renderApprovalPrompt({
		...PROCESS_FACTS,
		requester: { kind: "agent", agent: "programmer", display: "Programmer" },
		server: { name: "web", command: "npm run dev", cwd: "app", persistent: true },
		relay: { protocol: "tcp", host_port: 3000, guest_port: 5173, host_addresses: ["0.0.0.0"] },
		warning: "Listening on 0.0.0.0 accepts connections from every network this computer is on.",
		session_available: false,
	});
	expect(server.lines[0]).toBe("Relay request from the Programmer");
	expect(server.lines).toContain('  Server "web" in a container of its own, kept running after Clyean exits');
	expect(server.lines).toContain("  Command: npm run dev   (in app)");
	expect(server.lines.some((line: string) => line.startsWith("  Warning: Listening on 0.0.0.0"))).toBe(true);
	expect(server.options).toEqual([APPROVAL_CHOICES.project, APPROVAL_CHOICES.once, APPROVAL_CHOICES.deny]);

	const unreadable = renderApprovalPrompt({ ...PROCESS_FACTS, permanent_available: false, file_problem: "relays.json does not parse" });
	expect(unreadable.options).toEqual([APPROVAL_CHOICES.once, APPROVAL_CHOICES.deny]);
	expect(unreadable.lines.at(-1)).toContain("relays.json does not parse");

	const signIn = renderApprovalPrompt({ ...PROCESS_FACTS, requester: { kind: "sign_in", provider: "openrouter.ai" } });
	expect(signIn.lines[0]).toBe("The User Assistant's sign-in to openrouter.ai needs a relay");
});

test("the overlay moves with the arrow keys, picks with Enter or a digit, and denies on Escape or timeout", async () => {
	const { approvalOverlay } = await importFresh(MODULE);
	const choices: string[] = [];
	const options = ["Allow once", "Deny"];
	const overlay = approvalOverlay(["Relay request"], options, (choice: string) => choices.push(choice), () => {}, 60_000);
	expect(overlay.render(80)).toEqual(["Relay request", "", "> 1. Allow once", "  2. Deny"]);
	overlay.handleInput("\u001b[B");
	expect(overlay.render(80)[3]).toBe("> 2. Deny");
	overlay.handleInput("\r");
	expect(choices).toEqual(["Deny"]);
	overlay.handleInput("1");
	expect(choices).toEqual(["Deny"]);

	const picked: string[] = [];
	approvalOverlay(["x"], options, (choice: string) => picked.push(choice), () => {}, 60_000).handleInput("1");
	expect(picked).toEqual(["Allow once"]);
	const escaped: string[] = [];
	approvalOverlay(["x"], options, (choice: string) => escaped.push(choice), () => {}, 60_000).handleInput("\u001b");
	expect(escaped).toEqual(["Deny"]);
	const late: string[] = [];
	approvalOverlay(["x"], options, (choice: string) => late.push(choice), () => {}, 20);
	await waitFor(() => late.length === 1, 1000, "the timeout");
	expect(late[0]).not.toBe("Allow once");
});

function overlayContext(pick: string) {
	const shown: any[] = [];
	const { ctx } = createFakeContext({ sessionId: "s1" });
	(ctx.ui as any).custom = async (factory: any, options: any) => {
		shown.push(options);
		return new Promise(resolve => {
			const component = factory({ requestRender() {} }, {}, {}, resolve);
			shown.push(component.render(100));
			component.handleInput(pick);
		});
	};
	return { ctx, shown };
}

test("approvals ask through an overlay, one at a time, and hold Herdr blocked meanwhile", async () => {
	const { RelayApprovals } = await importFresh(MODULE);
	const emitted: any[] = [];
	const signal = { begin: (label: string) => emitted.push({ active: true, label }), clear: () => emitted.push({ active: false }) };
	const approvals = new RelayApprovals(signal, 60_000);
	const { ctx, shown } = overlayContext("1");
	expect(await approvals.approve(ctx, PROCESS_FACTS)).toEqual({ decision: "project" });
	expect(shown[0]).toEqual({ overlay: true });
	expect(emitted).toEqual([{ active: true, label: "Waiting for relay approval" }, { active: false }]);
	const denied = overlayContext("4");
	expect(await approvals.approve(denied.ctx, PROCESS_FACTS)).toEqual({ decision: "deny", reason: "denied" });
	expect(await approvals.approve({ hasUI: false }, PROCESS_FACTS)).toEqual({ decision: "deny", reason: "no_user_interface" });
});

function leaseReply(request: any): StubReply {
	if (request.method === "session.lease") return { result: { type: "lease" }, closeAfterEvents: false };
	return { result: { type: "project_status", scaffolded: true, incomplete_work: [] } };
}

async function installWithLease() {
	server = await startStubOrchestrator("relay-lease", leaseReply);
	sandbox.reset({
		CLYEAN_AGENT: "user-assistant",
		CLYEAN_ORCHESTRATOR_SOCKET: server.socketPath,
		CLYEAN_ORCHESTRATOR_CONNECT_TIMEOUT_MS: "500",
		CLYEAN_ORCHESTRATOR_LEASE: "1",
	});
	const fake = createFakePi();
	const { default: install } = await importFresh(MODULE);
	const quiet = { socketTables: () => [], fingerprint: async () => undefined, pollMs: 1000, windowMs: 1000, closeDelayMs: 10 };
	install(fake.pi, undefined, quiet);
	await waitFor(() => server!.requests.some(request => request.method === "session.lease"), 2000, "the lease");
	return fake;
}

test("the lease answers relay prompts and session queries, denying prompts without a user interface", async () => {
	const fake = await installWithLease();
	const { ctx } = createFakeContext({ hasUI: false, sessionId: "session-7" });
	await fake.emit("session_start", {}, ctx);
	server!.send({ id: "q1", method: "session.current", params: {} });
	server!.send({ id: "q2", method: "relay.approve", params: PROCESS_FACTS });
	await waitFor(() => server!.followUps.length === 2, 2000, "the answers");
	const answers = Object.fromEntries(server!.followUps.map(frame => [frame.id, frame]));
	expect(answers.q1.result).toEqual({ session_id: "session-7" });
	expect(answers.q2.result).toEqual({ decision: "deny", reason: "no_user_interface" });
});

test("/clyean relays edits the allowlist over the lease and shows the result", async () => {
	const fake = await installWithLease();
	const { ctx, notifications } = createFakeContext({ sessionId: "session-7" });
	await fake.emit("session_start", {}, ctx);
	const command = fake.commands.get("clyean").handler("relays allow project tcp,udp 3000-3010 any 127.0.0.1,::1", ctx);
	await waitFor(() => server!.followUps.length === 1, 2000, "the allowlist request");
	const sent = server!.followUps[0];
	expect(sent.method).toBe("relay.allowlist.add");
	expect(sent.params).toEqual({
		scope: "project",
		entry: { protocols: ["tcp", "udp"], hostPorts: "3000-3010", guestPorts: "any", hostAddresses: ["127.0.0.1", "::1"] },
		session_id: "session-7",
	});
	server!.send({
		id: sent.id,
		result: {
			type: "relay_allowlist",
			enabled: true,
			file: "/home/u/.config/clyean/relays.json",
			project_dir: "/p",
			project: [{ text: "tcp,udp 3000-3010 -> any on 127.0.0.1, ::1" }],
			session: [],
			relays: [],
		},
	});
	await command;
	expect(notifications.at(-1)?.message).toContain("  1. tcp,udp 3000-3010 -> any on 127.0.0.1, ::1");
	await fake.commands.get("clyean").handler("relays allow everywhere tcp 1 1 x", ctx);
	expect(notifications.at(-1)?.type).toBe("error");
	expect(notifications.at(-1)?.message).toContain("usage: /clyean relays allow");
});

test("only listening loopback and wildcard sockets count", async () => {
	const { listeningPorts } = await importFresh(MODULE);
	const tcp = [
		"  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode",
		"   0: 0100007F:D595 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1",
		"   1: 00000000:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2",
		"   2: 0100007F:1F90 0100007F:9C40 01 00000000:00000000 00:00000000 00000000     0        0 3",
		"   3: 0A00020F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 4",
	].join("\n");
	expect([...listeningPorts(tcp)].sort()).toEqual([3000, 54677]);
	const tcp6 = [
		"  sl  local_address                         remote_address                        st",
		"   0: 00000000000000000000000001000000:D595 00000000000000000000000000000000:0000 0A 00000000:00000000",
	].join("\n");
	expect([...listeningPorts(tcp6)]).toEqual([54677]);
});

test("sign-ins start with the sign-in commands only", async () => {
	const { startsSignIn } = await importFresh(MODULE);
	for (const text of ["/login", "/login openrouter", "/mcp add tracker", "/model", "/clyean sign-in sed tracker"]) {
		expect(startsSignIn(text)).toBe(true);
	}
	for (const text of ["/clyean", "/clyean relays", "log in to github", "/logout"]) expect(startsSignIn(text)).toBe(false);
});

test("the watcher relays a new callback server, and closes the relay once it stops listening", async () => {
	const { SignInWatcher } = await importFresh(MODULE);
	const header = "  sl  local_address rem_address   st";
	let listening = [3000];
	const table = () => [header, ...listening.map((port, index) => `   ${index}: 0100007F:${port.toString(16).toUpperCase().padStart(4, "0")} 00000000:0000 0A`)].join("\n");
	const requests: Array<{ method: string; params: any }> = [];
	const lease = {
		request: async (method: string, params: any) => {
			requests.push({ method, params });
			return method === "relay.open" ? { relay_id: "r-9" } : {};
		},
	};
	const notices: string[] = [];
	const blocked: string[] = [];
	const watcher = new SignInWatcher(
		{
			socketTables: () => [table()],
			fingerprint: async (port: number) => (port === 54549 ? { provider: "openrouter.ai" } : undefined),
			pollMs: 5,
			windowMs: 5000,
			closeDelayMs: 30,
		},
		() => lease,
		(message: string) => notices.push(message),
		{ begin: (label: string) => blocked.push(`+${label}`), clear: () => blocked.push("-") },
	);
	watcher.open();
	listening = [3000, 8080, 54549];
	await waitFor(() => requests.length === 1, 2000, "the relay request");
	expect(requests[0]).toEqual({
		method: "relay.open",
		params: { protocol: "tcp", host_port: 54549, guest_port: 54549, purpose: "sign-in", provider: "openrouter.ai" },
	});
	expect(notices[0]).toBe("Browser sign-in relayed through http://localhost:54549.");
	expect(blocked).toEqual(["+Waiting for browser sign-in"]);
	listening = [3000, 8080];
	await waitFor(() => requests.length === 2, 2000, "the relay to close");
	expect(requests[1]).toEqual({ method: "relay.close", params: { relay_id: "r-9" } });
	expect(blocked).toEqual(["+Waiting for browser sign-in", "-"]);
	watcher.close();
	expect(watcher.watching).toBe(false);
});

test("clyean_relay opens, lists, and closes relays through the orchestrator socket", async () => {
	server = await startStubOrchestrator("relay-tool", request => {
		if (request.method === "relay.open") {
			return {
				result: {
					type: "relay_open",
					relay_id: "r-1",
					protocol: "tcp",
					host_port: 3000,
					guest_port: 5173,
					host_addresses: ["127.0.0.1", "::1"],
					approval: "session",
					endpoints: ["127.0.0.1:3000", "[::1]:3000"],
				},
			};
		}
		if (request.method === "relay.list") return { result: { type: "relay_list", relays: [] } };
		if (request.method === "relay.close") return { result: { type: "relay_closed", relay_id: "r-1" } };
		return { error: { code: "unknown_method", message: "nope" } };
	});
	sandbox.reset({ CLYEAN_AGENT: "user-assistant", CLYEAN_ORCHESTRATOR_SOCKET: server.socketPath });
	const fake = createFakePi();
	const { default: install } = await importFresh(MODULE);
	install(fake.pi);
	const tool = fake.tools.get("clyean_relay");
	const opened = await tool.execute("c1", { action: "open", host_port: 3000, guest_port: 5173, reason: "preview" });
	expect(server.requests[0].params).toEqual({ protocol: "tcp", host_port: 3000, guest_port: 5173, reason: "preview" });
	expect(opened.content[0].text).toBe("Relay r-1 open: TCP 127.0.0.1:3000, [::1]:3000 -> port 5173 in this container (allowed for the session).");
	const listed = await tool.execute("c2", { action: "list" });
	expect(listed.content[0].text).toBe("No relays are open.");
	const closed = await tool.execute("c3", { action: "close", relay_id: "r-1" });
	expect(closed.content[0].text).toBe("Relay r-1 is closed.");
	const missing = await tool.execute("c4", { action: "open" });
	expect(missing.isError).toBe(true);
});
