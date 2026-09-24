// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

import { afterEach, beforeEach, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
	createFakeContext,
	createFakePi,
	environmentSandbox,
	importFresh,
	shortSocketPath,
	sleep,
	startStubOrchestrator,
	type StubOrchestrator,
	type StubReply,
	waitFor,
} from "./harness";

const BRIDGE_MODULE = "../clyean-orchestration.ts";
const TOOL_NAMES = [
	"clyean_status",
	"clyean_scaffold",
	"clyean_delegate",
	"clyean_provide_information",
	"clyean_resume",
	"clyean_cancel",
];

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

async function installBridge(handle: (request: any) => StubReply | Promise<StubReply>, socketPathOverride?: string) {
	if (!socketPathOverride) server = await startStubOrchestrator("orch", handle);
	sandbox.reset({
		CLYEAN_AGENT: "user-assistant",
		CLYEAN_ORCHESTRATOR_SOCKET: socketPathOverride ?? server!.socketPath,
		CLYEAN_ORCHESTRATOR_CONNECT_TIMEOUT_MS: "1000",
	});
	const fake = createFakePi();
	const { default: install } = await importFresh(BRIDGE_MODULE);
	install(fake.pi);
	return fake;
}

function statusReply(incompleteWork: unknown[] = []): StubReply {
	return {
		result: {
			type: "project_status",
			scaffolded: true,
			project_type: "SOFTWARE_ENGINEERING_PROJECT",
			locked: false,
			incomplete_work: incompleteWork,
		},
	};
}

function resultText(result: any): string {
	return result.content.map((block: any) => block.text).join("\n");
}

async function runTool(fake: ReturnType<typeof createFakePi>, name: string, params: unknown, ctx: unknown) {
	const updates: any[] = [];
	const tool = fake.tools.get(name);
	expect(tool).toBeDefined();
	const result = await tool.execute("call-1", params, undefined, (partial: any) => updates.push(partial), ctx);
	return { result, updates };
}

test("registers every orchestration tool and the /clyean command", async () => {
	const fake = await installBridge(() => statusReply());
	expect([...fake.tools.keys()].sort()).toEqual([...TOOL_NAMES].sort());
	expect(fake.commands.has("clyean")).toBe(true);
	expect(fake.tools.get("clyean_delegate").parameters.shape.prompt_type.values).toEqual([
		"SOFTWARE_ENGINEERING_PROJECT_RESEARCH",
		"SOFTWARE_ENGINEERING_PROJECT_PLANNING",
		"SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION",
	]);
	expect(fake.tools.get("clyean_delegate").parameters.shape.plan.optionalFlag).toBe(true);
});

test("agents other than the User Assistant register nothing", async () => {
	server = await startStubOrchestrator("orch", () => statusReply());
	sandbox.reset({ CLYEAN_AGENT: "programmer", CLYEAN_ORCHESTRATOR_SOCKET: server.socketPath });
	const fake = createFakePi();
	const { default: install } = await importFresh(BRIDGE_MODULE);
	install(fake.pi);
	expect(fake.tools.size).toBe(0);
	expect(fake.commands.size).toBe(0);
	expect(fake.handlers.size).toBe(0);
});

test("clyean_delegate streams progress, ignores replayed sequence numbers, and returns the completion", async () => {
	const fake = await installBridge(request => {
		if (request.method !== "work.start") return { error: { code: "unknown_method", message: `unknown method: ${request.method}` } };
		return {
			result: { type: "work_accepted", work_id: "w-plan" },
			events: [
				{ event: "progress", work_id: "w-plan", seq: 1, agent: "orchestrator", phase: "planning.overview", text: "Starting" },
				{ event: "agent_output", work_id: "w-plan", seq: 2, agent: "software-engineering-director", phase: "planning.overview", text: "Drafting overview" },
				{ event: "agent_output", work_id: "w-plan", seq: 2, agent: "software-engineering-director", phase: "planning.overview", text: "replayed" },
				{ event: "completed", work_id: "w-plan", seq: 3, summary: "The change plan is authored.", artifacts: [".clyean/plans/2026-09-21-add-login/v1.md"], plan: "2026-09-21-add-login/v1" },
			],
			eventDelayMs: 5,
		};
	});
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result, updates } = await runTool(
		fake,
		"clyean_delegate",
		{
			prompt_type: "SOFTWARE_ENGINEERING_PROJECT_PLANNING",
			refined_prompt: "Plan the login feature with OAuth.",
			original_prompt: "add login",
			plan: "2026-09-21-add-login/v1",
		},
		ctx,
	);
	expect(server!.requests[0].method).toBe("work.start");
	expect(server!.requests[0].params).toEqual({
		prompt_type: "SOFTWARE_ENGINEERING_PROJECT_PLANNING",
		prompt: "Plan the login feature with OAuth.",
		original_prompt: "add login",
		session_id: "ua-session",
		plan: "2026-09-21-add-login/v1",
	});
	expect(updates).toHaveLength(2);
	expect(resultText(updates[0])).toBe("[orchestrator/planning.overview] Starting");
	expect(resultText(updates[1])).toBe(
		"[orchestrator/planning.overview] Starting\n[software-engineering-director/planning.overview] Drafting overview",
	);
	expect(updates[1].details).toEqual({ status: "running", work_id: "w-plan" });
	expect(result.isError).toBeUndefined();
	const text = resultText(result);
	expect(text).toContain("Work w-plan completed.");
	expect(text).toContain("The change plan is authored.");
	expect(text).toContain("Change plan: 2026-09-21-add-login/v1");
	expect(text).toContain(".clyean/plans/2026-09-21-add-login/v1.md");
	expect(text).not.toContain("replayed");
	expect(result.details).toEqual({
		status: "completed",
		work_id: "w-plan",
		summary: "The change plan is authored.",
		artifacts: [".clyean/plans/2026-09-21-add-login/v1.md"],
		plan: "2026-09-21-add-login/v1",
	});
	expect(fake.busEmissions).toEqual([{ channel: "herdr:blocked", data: { active: true, label: "Change plan ready for review" } }]);
	await fake.emit("input", { type: "input", text: "looks good, implement it", source: "interactive" }, ctx);
	expect(fake.busEmissions[1]).toEqual({ channel: "herdr:blocked", data: { active: false, label: "Change plan ready for review" } });
});

test("an information request instructs the model to ask the user and continue, and blocks Herdr until the next input", async () => {
	const fake = await installBridge(() => ({
		result: { type: "work_accepted", work_id: "w-impl" },
		events: [
			{ event: "progress", work_id: "w-impl", seq: 1, agent: "orchestrator", phase: "planning.overview", text: "Reviewing prompt" },
			{ event: "information_requested", work_id: "w-impl", seq: 2, request_id: "req-7", questions: ["Which database?", "Which auth provider?"], context: "The overview needs both decisions." },
		],
	}));
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result, updates } = await runTool(
		fake,
		"clyean_delegate",
		{ prompt_type: "SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION", refined_prompt: "Implement login", original_prompt: "implement login" },
		ctx,
	);
	expect(updates).toHaveLength(1);
	expect(result.isError).toBeUndefined();
	const text = resultText(result);
	expect(text).toContain("`ask` tool");
	expect(text).toContain("`clyean_provide_information`");
	expect(text).toContain('work_id "w-impl"');
	expect(text).toContain('request_id "req-7"');
	expect(text).toContain("1. Which database?");
	expect(text).toContain("2. Which auth provider?");
	expect(text).toContain("The overview needs both decisions.");
	expect(result.details).toEqual({
		status: "information_requested",
		work_id: "w-impl",
		request_id: "req-7",
		questions: ["Which database?", "Which auth provider?"],
		context: "The overview needs both decisions.",
	});
	expect(fake.busEmissions).toEqual([{ channel: "herdr:blocked", data: { active: true, label: "Awaiting answers for the orchestrator" } }]);
	await fake.emit("input", { type: "input", text: "postgres and okta", source: "interactive" }, ctx);
	expect(fake.busEmissions).toHaveLength(2);
	expect(fake.busEmissions[1].data).toEqual({ active: false, label: "Awaiting answers for the orchestrator" });
});

test("clyean_provide_information resumes the work and streams it to completion", async () => {
	const fake = await installBridge(request => {
		expect(request.method).toBe("work.provide_information");
		expect(request.params).toEqual({ work_id: "w-impl", request_id: "req-7", answers: ["postgres", "okta"] });
		return {
			result: { type: "work_resumed", work_id: "w-impl" },
			events: [
				{ event: "progress", work_id: "w-impl", seq: 3, agent: "specifier", phase: "planning.specification", text: "Writing specification changes" },
				{ event: "completed", work_id: "w-impl", seq: 4, summary: "Implementation complete.", artifacts: ["src/login.ts", ".clyean/SPECS.md"] },
			],
		};
	});
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result, updates } = await runTool(
		fake,
		"clyean_provide_information",
		{ work_id: "w-impl", request_id: "req-7", answers: ["postgres", "okta"] },
		ctx,
	);
	expect(updates).toHaveLength(1);
	expect(resultText(result)).toContain("Implementation complete.");
	expect(resultText(result)).toContain("  - src/login.ts");
	expect(resultText(result)).toContain("Progress log:");
	expect(result.details.status).toBe("completed");
	expect(fake.busEmissions).toHaveLength(0);
});

test("a failed terminal event becomes a tool error that keeps the progress log", async () => {
	const fake = await installBridge(() => ({
		result: { type: "work_resumed", work_id: "w-9" },
		events: [
			{ event: "progress", work_id: "w-9", seq: 1, agent: "programmer", phase: "implementation.programming", text: "Compiling" },
			{ event: "failed", work_id: "w-9", seq: 2, code: "cancelled", message: "The user cancelled the work." },
		],
	}));
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result } = await runTool(fake, "clyean_resume", { work_id: "w-9" }, ctx);
	expect(result.isError).toBe(true);
	expect(resultText(result)).toContain("Work w-9 failed (cancelled): The user cancelled the work.");
	expect(resultText(result)).toContain("[programmer/implementation.programming] Compiling");
	expect(result.details).toEqual({ status: "failed", work_id: "w-9", code: "cancelled", message: "The user cancelled the work." });
});

test("an error response surfaces its message as a tool error", async () => {
	const fake = await installBridge(() => ({ error: { code: "project_locked", message: "another unit of work holds the project lock" } }));
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result } = await runTool(
		fake,
		"clyean_delegate",
		{ prompt_type: "SOFTWARE_ENGINEERING_PROJECT_RESEARCH", refined_prompt: "How does auth work?", original_prompt: "auth?" },
		ctx,
	);
	expect(result.isError).toBe(true);
	expect(resultText(result)).toBe("another unit of work holds the project lock");
});

test("clyean_status renders the project status including incomplete work", async () => {
	const fake = await installBridge(request => {
		expect(request.method).toBe("project.status");
		return statusReply([
			{ work_id: "w-1", prompt_type: "SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION", session_id: "s-1", phase: "implementation.programming", started_at: "2026-09-21T12:00:00Z" },
		]);
	});
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result } = await runTool(fake, "clyean_status", {}, ctx);
	const text = resultText(result);
	expect(text).toContain("Project scaffolded: yes");
	expect(text).toContain("Project type: SOFTWARE_ENGINEERING_PROJECT");
	expect(text).toContain("Project locked: no");
	expect(text).toContain("Incomplete work: 1");
	expect(text).toContain("  - w-1 (SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION, phase implementation.programming, started 2026-09-21T12:00:00Z, session s-1)");
	expect(result.details.type).toBe("project_status");
});

test("clyean_scaffold sends the project type and session id and streams the scaffold", async () => {
	const fake = await installBridge(request => {
		expect(request.method).toBe("project.scaffold");
		expect(request.params).toEqual({ project_type: "MISCELLANEOUS_PROJECT", session_id: "ua-session" });
		return {
			result: { type: "work_accepted", work_id: "scaffold-1" },
			events: [
				{ event: "progress", work_id: "scaffold-1", seq: 1, agent: "scaffolder", phase: "scaffold.git", text: "Initialized repository" },
				{ event: "completed", work_id: "scaffold-1", seq: 2, summary: "Scaffolding complete.", artifacts: [".clyean/project.json"] },
			],
		};
	});
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result, updates } = await runTool(fake, "clyean_scaffold", { project_type: "MISCELLANEOUS_PROJECT" }, ctx);
	expect(updates).toHaveLength(1);
	expect(resultText(result)).toContain("Scaffolding complete.");
	expect(fake.busEmissions).toHaveLength(0);
});

test("clyean_cancel reports the cancellation", async () => {
	const fake = await installBridge(request => {
		expect(request.method).toBe("work.cancel");
		return { result: { type: "work_cancelled", work_id: request.params.work_id } };
	});
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result } = await runTool(fake, "clyean_cancel", { work_id: "w-3" }, ctx);
	expect(resultText(result)).toBe("Work w-3 was cancelled.");
	expect(result.details).toEqual({ type: "work_cancelled", work_id: "w-3" });
});

test("an unreachable orchestrator yields an error naming the socket path", async () => {
	const missingSocket = shortSocketPath("missing");
	const fake = await installBridge(() => statusReply(), missingSocket);
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result } = await runTool(fake, "clyean_status", {}, ctx);
	expect(result.isError).toBe(true);
	expect(resultText(result)).toContain(missingSocket);
	expect(resultText(result)).toContain("not reachable");
	const delegate = await runTool(
		fake,
		"clyean_delegate",
		{ prompt_type: "SOFTWARE_ENGINEERING_PROJECT_RESEARCH", refined_prompt: "x", original_prompt: "x" },
		ctx,
	);
	expect(delegate.result.isError).toBe(true);
	expect(resultText(delegate.result)).toContain("not reachable");
});

test("a connection that closes before a terminal event is reported as an error", async () => {
	const fake = await installBridge(() => ({
		result: { type: "work_accepted", work_id: "w-cut" },
		events: [{ event: "progress", work_id: "w-cut", seq: 1, agent: "orchestrator", phase: "x", text: "partial" }],
	}));
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	const { result } = await runTool(fake, "clyean_resume", { work_id: "w-cut" }, ctx);
	expect(result.isError).toBe(true);
	expect(resultText(result)).toContain("before the work reached a terminal event");
});

test("session start with incomplete work notifies the user and queues a resume instruction", async () => {
	const fake = await installBridge(() =>
		statusReply([
			{ work_id: "w-1", prompt_type: "SOFTWARE_ENGINEERING_PROJECT_PLANNING", phase: "planning.specification" },
			{ work_id: "w-2", prompt_type: "SOFTWARE_ENGINEERING_PROJECT_RESEARCH" },
		]),
	);
	const { ctx, notifications } = createFakeContext({ sessionId: "ua-session" });
	await fake.emit("session_start", { type: "session_start" }, ctx);
	expect(notifications).toEqual([
		{ message: "Clyean found 2 unfinished units of work (w-1, w-2). Run /clyean for details or ask to resume.", type: "warning" },
	]);
	expect(fake.sentMessages).toHaveLength(1);
	expect(fake.sentMessages[0].options).toEqual({ deliverAs: "nextTurn" });
	expect(fake.sentMessages[0].message.customType).toBe("clyean-incomplete-work");
	expect(fake.sentMessages[0].message.display).toBe(false);
	expect(fake.sentMessages[0].message.content).toContain("- w-1: SOFTWARE_ENGINEERING_PROJECT_PLANNING, phase planning.specification");
	expect(fake.sentMessages[0].message.content).toContain("clyean_resume");
});

test("session start with no incomplete work is quiet, and an absent orchestrator is tolerated", async () => {
	const fake = await installBridge(() => statusReply());
	const { ctx, notifications } = createFakeContext({ sessionId: "ua-session" });
	await fake.emit("session_start", { type: "session_start" }, ctx);
	expect(notifications).toHaveLength(0);
	expect(fake.sentMessages).toHaveLength(0);
	const missingSocket = shortSocketPath("absent");
	const lonely = await installBridge(() => statusReply(), missingSocket);
	const lonelyContext = createFakeContext({ sessionId: "ua-session" });
	await lonely.emit("session_start", { type: "session_start" }, lonelyContext.ctx);
	expect(lonelyContext.notifications).toHaveLength(0);
});

test("the /clyean command prints the project status", async () => {
	const fake = await installBridge(() => statusReply([{ work_id: "w-5", prompt_type: "SOFTWARE_ENGINEERING_PROJECT_RESEARCH" }]));
	const { ctx, notifications } = createFakeContext({ sessionId: "ua-session" });
	await fake.commands.get("clyean").handler("", ctx);
	expect(notifications).toHaveLength(1);
	expect(notifications[0].type).toBe("info");
	expect(notifications[0].message).toContain("Incomplete work: 1");
	expect(notifications[0].message).toContain("  - w-5 (SOFTWARE_ENGINEERING_PROJECT_RESEARCH)");
});

test("a planning work resumed after restart still signals plan review on completion", async () => {
	const fake = await installBridge(request => {
		if (request.method === "project.status") {
			return statusReply([{ work_id: "w-plan", prompt_type: "SOFTWARE_ENGINEERING_PROJECT_PLANNING", phase: "planning.architecture" }]);
		}
		return {
			result: { type: "work_resumed", work_id: "w-plan" },
			events: [{ event: "completed", work_id: "w-plan", seq: 9, summary: "Plan finished.", plan: "2026-09-21-x/v2" }],
		};
	});
	const { ctx } = createFakeContext({ sessionId: "ua-session" });
	await fake.emit("session_start", { type: "session_start" }, ctx);
	const { result } = await runTool(fake, "clyean_resume", { work_id: "w-plan" }, ctx);
	expect(resultText(result)).toContain("Change plan: 2026-09-21-x/v2");
	expect(fake.busEmissions).toEqual([{ channel: "herdr:blocked", data: { active: true, label: "Change plan ready for review" } }]);
});

function leaseReply(request: any): StubReply {
	if (request.method === "session.lease") return { result: { type: "lease" }, closeAfterEvents: false };
	return statusReply();
}

function shutdownRecorder() {
	const { ctx, notifications } = createFakeContext();
	const recorder = { ctx: ctx as any, notifications, shutdowns: 0 };
	recorder.ctx.shutdown = () => {
		recorder.shutdowns += 1;
	};
	return recorder;
}

async function installWithLease(socketPath: string, harness?: () => Promise<any>) {
	sandbox.reset({
		CLYEAN_AGENT: "user-assistant",
		CLYEAN_ORCHESTRATOR_SOCKET: socketPath,
		CLYEAN_ORCHESTRATOR_CONNECT_TIMEOUT_MS: "500",
		CLYEAN_ORCHESTRATOR_LEASE: "1",
	});
	const fake = createFakePi();
	const { default: install } = await importFresh(BRIDGE_MODULE);
	install(fake.pi, harness);
	return fake;
}

function leaseRequests(): any[] {
	return server!.requests.filter(request => request.method === "session.lease");
}

test("the lease shuts the harness down when the clyean process that started it goes away", async () => {
	server = await startStubOrchestrator("lease", leaseReply);
	const fake = await installWithLease(server.socketPath);
	const recorder = shutdownRecorder();
	await fake.emit("session_start", {}, recorder.ctx);
	await waitFor(() => leaseRequests().length === 1, 2000, "the lease request");
	await sleep(50);
	expect(recorder.shutdowns).toBe(0);
	server.dropConnections();
	await waitFor(() => recorder.shutdowns === 1, 2000, "the shutdown");
	expect(recorder.notifications.at(-1)?.message).toContain("connection to the clyean process");
});

test("a lease that cannot reach the orchestrator shuts the harness down when the session starts", async () => {
	const fake = await installWithLease(shortSocketPath("absent"));
	await sleep(50);
	const recorder = shutdownRecorder();
	await fake.emit("session_start", {}, recorder.ctx);
	await waitFor(() => recorder.shutdowns === 1, 2000, "the shutdown");
});

test("no lease is held without CLYEAN_ORCHESTRATOR_LEASE", async () => {
	const fake = await installBridge(leaseReply);
	await fake.emit("session_start", {}, shutdownRecorder().ctx);
	await sleep(50);
	expect(leaseRequests()).toHaveLength(0);
});

test("loading the extension again keeps the one lease", async () => {
	server = await startStubOrchestrator("lease-reload", leaseReply);
	await installWithLease(server.socketPath);
	await installWithLease(server.socketPath);
	await waitFor(() => leaseRequests().length === 1, 2000, "the lease request");
	await sleep(50);
	expect(leaseRequests()).toHaveLength(1);
});

const MINUTE = 60_000;

function fakeAuthorityHarness(overrides: Record<string, unknown> = {}) {
	const calls = { modelRefreshes: [] as string[], mcpRefreshes: [] as string[], signIns: [] as string[] };
	const harness = {
		envApiKeyName: (provider: string) => (provider === "openai" ? "OPENAI_API_KEY" : undefined),
		refreshModelCredential: async (provider: string, credential: any) => {
			calls.modelRefreshes.push(provider);
			return { ...credential, access: `${credential.access}-renewed`, refresh: "rotated", expires: Date.now() + 60 * MINUTE };
		},
		refreshMcpCredential: async (credential: any, serverUrl: string) => {
			calls.mcpRefreshes.push(serverUrl);
			return { access: "mcp-renewed", refresh: "mcp-rotated", expires: Date.now() + 60 * MINUTE };
		},
		isDefinitiveOAuthFailure: (message: string) => message.includes("invalid_grant"),
		mcpCredentialId: (serverUrl: string, profile: string) => `mcp_oauth:profile:${profile}:${serverUrl}`,
		signInToMcpServer: async (serverUrl: string) => {
			calls.signIns.push(serverUrl);
			return { type: "oauth", access: "signed-in", refresh: "r", expires: Date.now() + 60 * MINUTE, tokenUrl: "https://auth/token" };
		},
		...overrides,
	};
	return { harness, calls };
}

/** A login store with the harness's lease-guarded refresh reduced to its effect. */
function fakeStore(rows: Record<string, Array<{ id: number; credential: any }>>, environmentProviders: string[] = []) {
	return {
		rows,
		listStoredCredentials: (provider: string) => rows[provider] ?? [],
		hasAuth: (provider: string) => environmentProviders.includes(provider) || (rows[provider]?.length ?? 0) > 0,
		set: async (provider: string, credential: any) => {
			rows[provider] = [{ id: 100, credential }];
		},
		refreshStoredOAuthCredential: async (provider: string, options: any) => {
			if (options.canRefresh && !options.canRefresh(options.observedCredential)) return;
			const refreshed = await options.refresh(options.observedCredential);
			const merged = options.mergeRefreshedCredential(options.observedCredential, refreshed);
			rows[provider] = rows[provider].map(row => (row.id === options.credentialId ? { id: row.id, credential: merged } : row));
		},
	};
}

test("copies are refreshed when they expire soon and never carry refresh tokens or client secrets", async () => {
	const { copyCredentials } = await importFresh(BRIDGE_MODULE);
	const { harness, calls } = fakeAuthorityHarness();
	const serverUrl = "https://mcp.example.com/sse";
	const store = fakeStore(
		{
			anthropic: [
				{ id: 1, credential: { type: "oauth", access: "soon", refresh: "r1", expires: Date.now() + 5 * MINUTE, email: "a@x" } },
				{ id: 2, credential: { type: "oauth", access: "later", refresh: "r2", expires: Date.now() + 50 * MINUTE } },
			],
			openai: [{ id: 3, credential: { type: "api_key", key: "sk-openai" } }],
			[`mcp_oauth:profile:user-assistant:${serverUrl}`]: [
				{
					id: 4,
					credential: { type: "oauth", access: "m", refresh: "mr", expires: Date.now() + MINUTE, tokenUrl: "https://auth/token", clientId: "c", clientSecret: "s" },
				},
			],
		},
		[],
	);
	const copies = await copyCredentials(store, harness, {
		agent: "programmer",
		providers: ["anthropic", "openai", "xai"],
		mcp_servers: [serverUrl],
	});
	expect(calls.modelRefreshes).toEqual(["anthropic"]);
	expect(calls.mcpRefreshes).toEqual([serverUrl]);
	expect(copies.providers.anthropic.map((copy: any) => copy.access)).toEqual(["soon-renewed", "later"]);
	expect(copies.providers.anthropic.every((copy: any) => copy.refresh === "")).toBe(true);
	expect(store.rows.anthropic[0].credential.refresh).toBe("rotated");
	expect(copies.providers.openai).toEqual([{ type: "api_key", key: "sk-openai" }]);
	const mcpCopy = copies.mcp[`mcp_oauth:profile:programmer:${serverUrl}`];
	expect(mcpCopy.access).toBe("mcp-renewed");
	expect(mcpCopy.refresh).toBe("");
	expect(mcpCopy.clientSecret).toBeUndefined();
	expect(mcpCopy.tokenUrl).toBe("https://auth/token");
	expect(copies.unavailable).toEqual(["xai"]);
});

test("patterns resolve through the harness, then the catalog, then the provider prefix", async () => {
	const { resolveModels } = await importFresh(BRIDGE_MODULE);
	const models = {
		resolve: (spec: string) => (spec.startsWith("gpt-5") ? { provider: "openai", id: "gpt-5" } : undefined),
		current: () => ({ provider: "openai", id: "gpt-5.5" }),
	};
	const catalog = [
		{ provider: "anthropic", id: "claude-sonnet-4-5" },
		{ provider: "google", id: "gemini-3-pro" },
	];
	const answer = resolveModels(models, catalog, ["gpt-5:high", "anthropic/claude-sonnet-4-5:max", "google/*", "unknown-model"]);
	expect(answer.models).toEqual({
		"gpt-5:high": { provider: "openai", model: "openai/gpt-5:high" },
		"anthropic/claude-sonnet-4-5:max": { provider: "anthropic", model: "anthropic/claude-sonnet-4-5:max" },
		"google/*": { provider: "google", model: null },
		"unknown-model": null,
	});
	expect(answer.current).toEqual({ provider: "openai", model: "openai/gpt-5.5" });
});

test("provider variables come from the harness catalog, with fixed lists for computed lookups", async () => {
	const { providerVariables } = await importFresh(BRIDGE_MODULE);
	const { harness } = fakeAuthorityHarness();
	const { variables } = providerVariables(harness, ["openai", "anthropic", "amazon-bedrock", "local-llm"]);
	expect(variables.openai).toEqual(["OPENAI_API_KEY"]);
	expect(variables.anthropic).toContain("ANTHROPIC_OAUTH_TOKEN");
	expect(variables["amazon-bedrock"]).toContain("AWS_SECRET_ACCESS_KEY");
	expect(variables["local-llm"]).toEqual([]);
});

test("the lease answers the orchestrator's credential requests from the session's login store", async () => {
	server = await startStubOrchestrator("lease-credentials", leaseReply);
	const { harness } = fakeAuthorityHarness();
	const fake = await installWithLease(server.socketPath, async () => harness);
	const recorder = shutdownRecorder();
	recorder.ctx.models = {
		resolve: (spec: string) => (spec === "gpt-5" ? { provider: "openai", id: "gpt-5" } : undefined),
		current: () => ({ provider: "openai", id: "gpt-5" }),
	};
	recorder.ctx.modelRegistry = {
		getAll: () => [],
		authStorage: fakeStore({ openai: [{ id: 1, credential: { type: "api_key", key: "sk" } }] }),
	};
	await waitFor(() => leaseRequests().length === 1, 2000, "the lease request");
	server.send({ id: "early", method: "credentials.variables", params: { providers: ["openai"] } });
	await waitFor(() => server!.followUps.length === 1, 2000, "the early answer");
	expect(server.followUps[0]).toEqual({ id: "early", error: { code: "not_ready", message: "the User Assistant's session has not started" } });

	await fake.emit("session_start", {}, recorder.ctx);
	server.send({ id: "c1", method: "credentials.resolve", params: { patterns: ["gpt-5"] } });
	server.send({ id: "c2", method: "credentials.copies", params: { agent: "programmer", providers: ["openai"], mcp_servers: [] } });
	server.send({ id: "c3", method: "credentials.nonsense", params: {} });
	await waitFor(() => server!.followUps.length === 4, 2000, "the answers");
	const answers = Object.fromEntries(server.followUps.map(frame => [frame.id, frame]));
	expect(answers.c1.result.models).toEqual({ "gpt-5": { provider: "openai", model: "openai/gpt-5" } });
	expect(answers.c2.result.providers).toEqual({ openai: [{ type: "api_key", key: "sk" }] });
	expect(answers.c3.error.message).toContain("unknown method credentials.nonsense");
	expect(recorder.shutdowns).toBe(0);
});

test("signing in for another agent reads its MCP servers and records the sign-in in this login store", async () => {
	const { signInOnBehalf } = await importFresh(BRIDGE_MODULE);
	const projectDir = fs.mkdtempSync(path.join(os.tmpdir(), "clyean-signin-"));
	try {
		const agents = path.join(projectDir, ".clyean", "agents");
		fs.mkdirSync(agents, { recursive: true });
		fs.writeFileSync(
			path.join(agents, "SOFTWARE_ENGINEERING_DIRECTOR.mcp.json"),
			JSON.stringify({ mcpServers: { tracker: { url: "https://tracker.example.com/mcp" }, local: { command: "x" } } }),
		);
		fs.writeFileSync(
			path.join(agents, "SOFTWARE_ENGINEERING_DIRECTOR.mcp.local.json"),
			JSON.stringify({ mcpServers: { tracker: { url: "https://tracker.internal/mcp" } } }),
		);
		const { harness, calls } = fakeAuthorityHarness();
		const store = fakeStore({});
		const callbacks = { onAuth: () => {}, onManualCodeInput: async () => "" };
		const url = await signInOnBehalf(
			{ agent: "software-engineering-director", server: "tracker", projectDir },
			store,
			harness,
			callbacks,
		);
		expect(url).toBe("https://tracker.internal/mcp");
		expect(calls.signIns).toEqual(["https://tracker.internal/mcp"]);
		expect(store.rows["mcp_oauth:profile:user-assistant:https://tracker.internal/mcp"][0].credential.access).toBe("signed-in");
		await expect(
			signInOnBehalf({ agent: "software-engineering-director", server: "local", projectDir }, store, harness, callbacks),
		).rejects.toThrow("not a remote MCP server");
		await expect(
			signInOnBehalf({ agent: "programmer", server: "tracker", projectDir }, store, harness, callbacks),
		).rejects.toThrow("has no MCP server named tracker");
	} finally {
		fs.rmSync(projectDir, { recursive: true, force: true });
	}
});
