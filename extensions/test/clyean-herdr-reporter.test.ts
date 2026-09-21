// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

import { afterEach, beforeEach, expect, test } from "bun:test";
import net from "node:net";
import {
	createFakeContext,
	createFakePi,
	environmentSandbox,
	importFresh,
	type RecordingServer,
	sleep,
	startRecordingServer,
	waitFor,
} from "./harness";

const REPORTER_MODULE = "../clyean-herdr-reporter.ts";
const PANE_ID = "w1:p1";
const IDENTITY = { pane_id: PANE_ID, source: "custom:clyean", agent: "clyean" };
const SESSION_FILE = "/home/skye/.omp/profiles/user-assistant/agent/sessions/s1.jsonl";
const HOST_ROOT = "/host/ws/proj/.clyean/container-root";

const sandbox = environmentSandbox();
const originalCreateConnection = net.createConnection;
let server: RecordingServer | undefined;

beforeEach(() => sandbox.reset());

afterEach(async () => {
	net.createConnection = originalCreateConnection;
	if (server) {
		await server.close();
		server = undefined;
	}
	sandbox.restore();
});

async function installReporter(
	environment: Record<string, string | undefined> = {},
	serverOptions: { dropFirstResponse?: boolean } = {},
) {
	server = await startRecordingServer("herdr", serverOptions);
	sandbox.reset({
		HERDR_ENV: "1",
		HERDR_PANE_ID: PANE_ID,
		HERDR_SOCKET_PATH: server.socketPath,
		CLYEAN_AGENT: "user-assistant",
		CLYEAN_PROJECT_DIR: "/home/skye/workspace/ws/proj",
		CLYEAN_HOST_CONTAINER_ROOT: HOST_ROOT,
		CLYEAN_HERDR_IDLE_DEBOUNCE_MS: "20",
		CLYEAN_HERDR_RETRY_GRACE_MS: "40",
		...environment,
	});
	const fake = createFakePi();
	const { default: install } = await importFresh(REPORTER_MODULE);
	install(fake.pi);
	return fake;
}

function stateFrames() {
	return server!.frames("pane.report_agent").map(frame => {
		const summary: { state: string; message?: string } = { state: frame.params.state };
		if (frame.params.message !== undefined) summary.message = frame.params.message;
		return summary;
	});
}

function withoutSeq(params: Record<string, unknown>) {
	const { seq, ...rest } = params;
	expect(typeof seq).toBe("number");
	return rest;
}

async function startedSession(fake: ReturnType<typeof createFakePi>, options = {}) {
	const { ctx } = createFakeContext({ sessionFile: SESSION_FILE, sessionId: "sess-1", ...options });
	await fake.emit("session_start", { type: "session_start" }, ctx);
	await waitFor(() => stateFrames().length >= 1, 2000, "initial idle report");
	return ctx;
}

test("session start reports pane metadata, the host session path, and idle, in that order", async () => {
	const fake = await installReporter();
	await startedSession(fake);
	await waitFor(() => server!.requests.length >= 3);
	expect(server!.requests.map(request => request.method)).toEqual([
		"pane.report_metadata",
		"pane.report_agent_session",
		"pane.report_agent",
	]);
	expect(withoutSeq(server!.requests[0].params)).toEqual({ ...IDENTITY, display_agent: "Clyean", title: "proj" });
	expect(withoutSeq(server!.requests[1].params)).toEqual({
		...IDENTITY,
		session_start_source: "startup",
		agent_session_path: `${HOST_ROOT}${SESSION_FILE}`,
	});
	expect(withoutSeq(server!.requests[2].params)).toEqual({
		...IDENTITY,
		state: "idle",
		agent_session_path: `${HOST_ROOT}${SESSION_FILE}`,
	});
	for (const request of server!.requests) expect(request.id).toStartWith("custom:clyean:");
});

test("session paths under the workspace mount translate to the host workspace path", async () => {
	const fake = await installReporter({
		CLYEAN_WORKSPACE_DIR: "/home/skye/workspace/ws",
		CLYEAN_HOST_WORKSPACE_DIR: "/host/ws",
	});
	await startedSession(fake, { sessionFile: "/home/skye/workspace/ws/proj/.omp/sessions/s2.jsonl" });
	const sessionFrame = server!.frames("pane.report_agent_session")[0];
	expect(sessionFrame.params.agent_session_path).toBe("/host/ws/proj/.omp/sessions/s2.jsonl");
	expect(sessionFrame.params.agent_session_id).toBeUndefined();
});

test("session paths with no host equivalent fall back to the session id", async () => {
	const fake = await installReporter();
	await startedSession(fake, { sessionFile: "/run/clyean/sessions/s3.jsonl", sessionId: "sess-3" });
	const sessionFrame = server!.frames("pane.report_agent_session")[0];
	expect(sessionFrame.params.agent_session_path).toBeUndefined();
	expect(sessionFrame.params.agent_session_id).toBe("sess-3");
	expect(stateFrames()).toEqual([{ state: "idle" }]);
	expect(server!.frames("pane.report_agent")[0].params.agent_session_id).toBe("sess-3");
});

test("session paths fall back to the session id when the host mapping variables are unset", async () => {
	const fake = await installReporter({ CLYEAN_HOST_CONTAINER_ROOT: undefined });
	await startedSession(fake, { sessionId: "sess-4" });
	const sessionFrame = server!.frames("pane.report_agent_session")[0];
	expect(sessionFrame.params).not.toHaveProperty("agent_session_path");
	expect(sessionFrame.params.agent_session_id).toBe("sess-4");
});

test("translateContainerPathToHost follows the sandbox contract mapping rules", async () => {
	const { translateContainerPathToHost } = await importFresh(REPORTER_MODULE);
	const env = {
		CLYEAN_WORKSPACE_DIR: "/home/skye/workspace/ws",
		CLYEAN_HOST_WORKSPACE_DIR: "/host/ws",
		CLYEAN_HOST_CONTAINER_ROOT: "/host/ws/proj/.clyean/container-root",
	};
	expect(translateContainerPathToHost("/home/skye/workspace/ws/a/b.jsonl", env)).toBe("/host/ws/a/b.jsonl");
	expect(translateContainerPathToHost("/home/skye/workspace/ws", env)).toBe("/host/ws");
	expect(translateContainerPathToHost("/home/skye/workspace/wsx/a", env)).toBe(
		"/host/ws/proj/.clyean/container-root/home/skye/workspace/wsx/a",
	);
	expect(translateContainerPathToHost("/home/skye/.omp/s.jsonl", env)).toBe(
		"/host/ws/proj/.clyean/container-root/home/skye/.omp/s.jsonl",
	);
	expect(translateContainerPathToHost("/run/clyean/x", env)).toBeUndefined();
	expect(translateContainerPathToHost("/mnt/data/x", env)).toBeUndefined();
	expect(translateContainerPathToHost("relative/x", env)).toBeUndefined();
	expect(translateContainerPathToHost("/home/skye/x", {})).toBeUndefined();
	expect(
		translateContainerPathToHost("/home/skye/workspace/ws/proj/s.jsonl", {
			...env,
			CLYEAN_HOST_WORKSPACE_DIR: "C:\\Users\\skye\\ws",
		}),
	).toBe("C:\\Users\\skye\\ws\\proj\\s.jsonl");
});

test("a turn reports working from agent_start and debounced idle after agent_end", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 2);
	expect(stateFrames()).toEqual([{ state: "idle" }, { state: "working" }]);
	await fake.emit("agent_end", { type: "agent_end", messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
	await sleep(5);
	expect(stateFrames()).toHaveLength(2);
	await waitFor(() => stateFrames().length >= 3, 1000, "debounced idle");
	expect(stateFrames()[2]).toEqual({ state: "idle" });
	const sessionReports = server!.frames("pane.report_agent_session");
	expect(sessionReports).toHaveLength(2);
	expect(sessionReports[1].params.session_start_source).toBe("startup");
});

test("overlapping blocks are reference counted and carry the latest reason", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 2);
	await fake.emit(
		"tool_approval_requested",
		{ type: "tool_approval_requested", sessionId: "sess-1", toolCallId: "c1", toolName: "bash", reason: "Run rm -rf build?" },
		ctx,
	);
	await waitFor(() => stateFrames().length >= 3);
	await fake.emit(
		"tool_execution_start",
		{ type: "tool_execution_start", toolCallId: "c2", toolName: "ask", args: { questions: [{ question: "Which database?" }] } },
		ctx,
	);
	await waitFor(() => stateFrames().length >= 4);
	await fake.emit(
		"tool_approval_resolved",
		{ type: "tool_approval_resolved", sessionId: "sess-1", toolCallId: "c1", toolName: "bash", approved: true },
		ctx,
	);
	await sleep(60);
	expect(stateFrames()).toEqual([
		{ state: "idle" },
		{ state: "working" },
		{ state: "blocked", message: "Run rm -rf build?" },
		{ state: "blocked", message: "Which database?" },
	]);
	await fake.emit("tool_execution_end", { type: "tool_execution_end", toolCallId: "c2", toolName: "ask", result: {}, isError: false }, ctx);
	await waitFor(() => stateFrames().length >= 5);
	expect(stateFrames()[4]).toEqual({ state: "working" });
	await fake.emit(
		"tool_approval_requested",
		{ type: "tool_approval_requested", sessionId: "sess-1", toolCallId: "c3", toolName: "edit", approvalMode: "write" },
		ctx,
	);
	await waitFor(() => stateFrames().length >= 6);
	expect(stateFrames()[5]).toEqual({ state: "blocked", message: "edit approval" });
});

test("herdr:blocked bus events count as blocks and clear when released", async () => {
	const fake = await installReporter();
	await startedSession(fake);
	fake.pi.events.emit("herdr:blocked", { active: true, label: "Change plan ready for review" });
	await waitFor(() => stateFrames().length >= 2);
	expect(stateFrames()[1]).toEqual({ state: "blocked", message: "Change plan ready for review" });
	fake.pi.events.emit("herdr:blocked", { active: false, label: "Change plan ready for review" });
	await waitFor(() => stateFrames().length >= 3);
	expect(stateFrames()[2]).toEqual({ state: "idle" });
});

test("a retryable provider failure holds working for the grace period and then reports blocked", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 2);
	await fake.emit(
		"agent_end",
		{ type: "agent_end", messages: [{ role: "assistant", stopReason: "error", errorMessage: "overloaded_error: Overloaded" }] },
		ctx,
	);
	await sleep(20);
	expect(stateFrames()).toEqual([{ state: "idle" }, { state: "working" }]);
	await waitFor(() => stateFrames().length >= 3, 1000, "retry hold to expire");
	expect(stateFrames()[2]).toEqual({ state: "blocked", message: "overloaded_error: Overloaded" });
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 4);
	expect(stateFrames()[3]).toEqual({ state: "working" });
});

test("a retry that resumes within the grace period never publishes idle or blocked", async () => {
	const fake = await installReporter({ CLYEAN_HERDR_RETRY_GRACE_MS: "200" });
	const ctx = await startedSession(fake);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 2);
	await fake.emit(
		"agent_end",
		{ type: "agent_end", messages: [{ role: "assistant", stopReason: "error", errorMessage: "429 rate limit" }] },
		ctx,
	);
	await sleep(30);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await fake.emit("agent_end", { type: "agent_end", messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
	await waitFor(() => stateFrames().length >= 3, 1000, "idle after successful retry");
	await sleep(250);
	expect(stateFrames()).toEqual([{ state: "idle" }, { state: "working" }, { state: "idle" }]);
});

test("willContinue, duplicate, and late agent_end events never publish a false idle", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 2);
	await fake.emit("agent_end", { type: "agent_end", messages: [], willContinue: true }, ctx);
	await sleep(60);
	expect(stateFrames()).toHaveLength(2);
	await fake.emit("agent_end", { type: "agent_end", messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
	await waitFor(() => stateFrames().length >= 3);
	await fake.emit("agent_end", { type: "agent_end", messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
	await fake.emit("agent_end", { type: "agent_end", messages: [{ role: "assistant", stopReason: "stop" }] }, ctx);
	await sleep(80);
	expect(stateFrames()).toEqual([{ state: "idle" }, { state: "working" }, { state: "idle" }]);
});

test("every report carries a strictly increasing sequence number", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await waitFor(() => stateFrames().length >= 2);
	await fake.emit(
		"tool_approval_requested",
		{ type: "tool_approval_requested", sessionId: "sess-1", toolCallId: "c1", toolName: "bash", reason: "approve?" },
		ctx,
	);
	await waitFor(() => stateFrames().length >= 3);
	await fake.emit(
		"tool_approval_resolved",
		{ type: "tool_approval_resolved", sessionId: "sess-1", toolCallId: "c1", toolName: "bash", approved: false },
		ctx,
	);
	await waitFor(() => stateFrames().length >= 4);
	await fake.emit("session_switch", { type: "session_switch", reason: "new", previousSessionFile: SESSION_FILE }, ctx);
	await waitFor(() => server!.requests.length >= 10, 2000, "all reports");
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await waitFor(() => server!.frames("pane.release_agent").length >= 1, 2000, "release");
	const sequencedRequests = server!.requests.filter(request => request.method !== "pane.release_agent");
	expect(sequencedRequests).toHaveLength(10);
	const sequences = sequencedRequests.map(request => request.params.seq as number);
	for (let index = 1; index < sequences.length; index += 1) {
		expect(sequences[index]).toBeGreaterThan(sequences[index - 1]);
	}
});

test("session switches re-report metadata and session identity with the switch reason", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("session_switch", { type: "session_switch", reason: "fork", previousSessionFile: SESSION_FILE }, ctx);
	await waitFor(() => server!.frames("pane.report_agent_session").length >= 2);
	expect(server!.frames("pane.report_agent_session")[1].params.session_start_source).toBe("fork");
	await waitFor(() => server!.frames("pane.report_metadata").length >= 2);
});

test("runtime rebinds (/new, /reload, /resume, /fork, /restart) never release the pane", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("session_switch", { type: "session_switch", reason: "new", previousSessionFile: SESSION_FILE }, ctx);
	await fake.emit("input", { type: "input", text: "/reload", source: "interactive" }, ctx);
	await fake.emit("session_switch", { type: "session_switch", reason: "resume", previousSessionFile: SESSION_FILE }, ctx);
	await fake.emit("input", { type: "input", text: "/fork", source: "interactive" }, ctx);
	await fake.emit("session_switch", { type: "session_switch", reason: "fork", previousSessionFile: SESSION_FILE }, ctx);
	await fake.emit("input", { type: "input", text: "  /restart", source: "interactive" }, ctx);
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await sleep(80);
	expect(server!.frames("pane.release_agent")).toHaveLength(0);
});

test("a genuine quit releases the pane exactly once, after the queued reports", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("input", { type: "input", text: "/exit", source: "interactive" }, ctx);
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await sleep(40);
	const releases = server!.frames("pane.release_agent");
	expect(releases).toHaveLength(1);
	expect(releases[0].params).toEqual({ pane_id: PANE_ID, source: "custom:clyean" });
	expect(server!.requests[server!.requests.length - 1].method).toBe("pane.release_agent");
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await sleep(40);
	expect(server!.frames("pane.release_agent")).toHaveLength(1);
});

test("a signal-driven shutdown without any input releases the pane", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await sleep(40);
	expect(server!.frames("pane.release_agent")).toHaveLength(1);
});

test("an abandoned /restart does not suppress the release of a later genuine quit", async () => {
	const fake = await installReporter();
	const ctx = await startedSession(fake);
	await fake.emit("input", { type: "input", text: "/restart", source: "interactive" }, ctx);
	await fake.emit("input", { type: "input", text: "keep going", source: "interactive" }, ctx);
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await sleep(40);
	expect(server!.frames("pane.release_agent")).toHaveLength(1);
});

test("agents other than the User Assistant register nothing and send nothing", async () => {
	const fake = await installReporter({ CLYEAN_AGENT: "programmer" });
	expect(fake.handlers.size).toBe(0);
	expect(fake.busHandlers.size).toBe(0);
	const { ctx } = createFakeContext({ sessionFile: SESSION_FILE, sessionId: "sess-1" });
	await fake.emit("session_start", { type: "session_start" }, ctx);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await sleep(60);
	expect(server!.requests).toHaveLength(0);
});

test("a context without a UI stays silent", async () => {
	const fake = await installReporter();
	const { ctx } = createFakeContext({ hasUI: false, sessionFile: SESSION_FILE, sessionId: "sess-1" });
	await fake.emit("session_start", { type: "session_start" }, ctx);
	await fake.emit("agent_start", { type: "agent_start" }, ctx);
	await fake.emit("tool_approval_requested", { type: "tool_approval_requested", toolName: "bash", reason: "x" }, ctx);
	await fake.emit("agent_end", { type: "agent_end", messages: [] }, ctx);
	await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	await sleep(80);
	expect(server!.requests).toHaveLength(0);
});

test("without the Herdr environment no socket connection is attempted and nothing fails", async () => {
	let connectionAttempts = 0;
	net.createConnection = ((...args: unknown[]) => {
		connectionAttempts += 1;
		return Reflect.apply(originalCreateConnection, net, args);
	}) as typeof net.createConnection;
	for (const environment of [
		{ CLYEAN_AGENT: "user-assistant" },
		{ CLYEAN_AGENT: "user-assistant", HERDR_SOCKET_PATH: "/tmp/never.sock", HERDR_BIN_PATH: "/usr/bin/herdr" },
		{ CLYEAN_AGENT: "user-assistant", HERDR_ENV: "1", HERDR_PANE_ID: PANE_ID },
		{ CLYEAN_AGENT: "user-assistant", HERDR_PANE_ID: PANE_ID, HERDR_SOCKET_PATH: "/tmp/never.sock" },
	]) {
		sandbox.reset(environment);
		const fake = createFakePi();
		const { default: install } = await importFresh(REPORTER_MODULE);
		install(fake.pi);
		const { ctx } = createFakeContext({ sessionFile: SESSION_FILE, sessionId: "sess-1" });
		await fake.emit("session_start", { type: "session_start" }, ctx);
		await fake.emit("agent_start", { type: "agent_start" }, ctx);
		await fake.emit("agent_end", { type: "agent_end", messages: [] }, ctx);
		await fake.emit("session_shutdown", { type: "session_shutdown" }, ctx);
	}
	await sleep(60);
	expect(connectionAttempts).toBe(0);
});

test("first-class Herdr support for Clyean silences the shipped reporter", async () => {
	const fake = await installReporter({ HERDR_CLYEAN_INTEGRATION: "1" });
	expect(fake.handlers.size).toBe(0);
});

test("a nested harness session does not report over the pane's root agent", async () => {
	const fake = await installReporter({ OMPCODE: "1" });
	expect(fake.handlers.size).toBe(0);
});

test("a report that gets no answer is retried exactly once", async () => {
	const fake = await installReporter({}, { dropFirstResponse: true });
	await startedSession(fake);
	await waitFor(() => server!.requests.length >= 3);
	expect(server!.connections).toBe(4);
	expect(server!.requests.map(request => request.method)).toEqual([
		"pane.report_metadata",
		"pane.report_agent_session",
		"pane.report_agent",
	]);
});

test("detection helpers follow the harness's own isInsideHerdr rules", async () => {
	const { isInsideHerdrPane, reportingEnabled } = await importFresh(REPORTER_MODULE);
	expect(isInsideHerdrPane({ HERDR_ENV: "1" })).toBe(true);
	expect(isInsideHerdrPane({ HERDR_PANE_ID: "w1:p1" })).toBe(true);
	expect(isInsideHerdrPane({ HERDR_TAB_ID: "t1" })).toBe(true);
	expect(isInsideHerdrPane({ HERDR_WORKSPACE_ID: "w1" })).toBe(true);
	expect(isInsideHerdrPane({ HERDR_SOCKET_PATH: "/x", HERDR_BIN_PATH: "/y", HERDR_SESSION: "s" })).toBe(false);
	expect(isInsideHerdrPane({ HERDR_ENV: "0" })).toBe(false);
	expect(reportingEnabled({ HERDR_ENV: "1", HERDR_PANE_ID: "w1:p1", HERDR_SOCKET_PATH: "/x" })).toBe(true);
	expect(reportingEnabled({ HERDR_ENV: "1", HERDR_PANE_ID: "w1:p1" })).toBe(false);
	expect(reportingEnabled({ HERDR_PANE_ID: "w1:p1", HERDR_SOCKET_PATH: "/x" })).toBe(false);
});
