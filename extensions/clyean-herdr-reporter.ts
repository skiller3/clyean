// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
// CLYEAN_EXTENSION_VERSION=1
// managed by clyean; upgrading clyean overwrites this file.
// add user customizations in sibling files instead of editing this one.
//
// Herdr agent-state reporter for the Clyean User Assistant.
//
// Derived from Herdr's own Oh-My-Pi integration asset (herdrdev/herdr,
// src/integration/assets/omp/herdr-agent-state.ts, integration version 10),
// which is licensed under the Apache License 2.0.  See NOTICE.  Clyean ships
// this file itself because `herdr integration install clyean` does not exist
// and the User Assistant runs inside a Podman container Herdr cannot see into.

import fs from "node:fs";
import net from "node:net";
import os from "node:os";
import path from "node:path";

type Env = Record<string, string | undefined>;
type AgentState = "working" | "blocked" | "idle";
type Timer = ReturnType<typeof setTimeout>;
type HandlerContext = {
	hasUI?: boolean;
	isIdle?: () => boolean;
	sessionManager?: {
		getSessionFile?: () => string | undefined;
		getSessionId?: () => string | undefined;
	};
};
type ExtensionApiLike = {
	on(event: string, handler: (event: any, ctx: any) => unknown): void;
	events: { on(channel: string, handler: (data: any) => void): unknown };
};

export const HERDR_SOURCE = "custom:clyean";
export const HERDR_AGENT_LABEL = "clyean";
export const HERDR_DISPLAY_AGENT = "Clyean";
export const USER_ASSISTANT_AGENT_ID = "user-assistant";
export const NATIVE_HERDR_INTEGRATION_FILE = "herdr-clyean-agent-state.ts";

const FIRST_ATTEMPT_TIMEOUT_MS = 500;
const RETRY_ATTEMPT_TIMEOUT_MS = 1500;
const RELEASE_ATTEMPT_TIMEOUT_MS = 500;
const HOST_INVISIBLE_PREFIXES = ["/run", "/mnt"];
const RESTART_COMMAND_PATTERN = /^\s*\/restart(?:\s|$)/;
const RETRYABLE_PROVIDER_ERROR_PATTERN =
	/overloaded|provider.?returned.?error|rate.?limit|too many requests|429|500|502|503|504|service.?unavailable|server.?error|internal.?error|network.?error|connection.?error|connection.?refused|connection.?lost|websocket.?closed|websocket.?error|other side closed|fetch failed|upstream.?connect|reset before headers|socket hang up|ended without|http2 request did not get a response|timed? out|timeout|terminated|retry delay/i;

export function isInsideHerdrPane(env: Env = process.env): boolean {
	if (env.HERDR_ENV === "1") return true;
	return !!env.HERDR_PANE_ID || !!env.HERDR_TAB_ID || !!env.HERDR_WORKSPACE_ID;
}

export function reportingEnabled(env: Env = process.env): boolean {
	return env.HERDR_ENV === "1" && !!env.HERDR_PANE_ID && !!env.HERDR_SOCKET_PATH;
}

export function isUserAssistantAgent(env: Env = process.env): boolean {
	return env.CLYEAN_AGENT === USER_ASSISTANT_AGENT_ID;
}

export function isNestedHarnessSession(env: Env = process.env): boolean {
	// The harness marks every shell it spawns with OMPCODE=1, so a nested
	// harness launched from a session's shell is not the pane's root agent.
	return env.OMPCODE === "1";
}

export function nativeHerdrSupportPresent(env: Env = process.env, homeDir: string = os.homedir()): boolean {
	if (env.HERDR_CLYEAN_INTEGRATION === "1") return true;
	const candidateDirectories = [
		path.join(homeDir, ".omp", "agent", "extensions"),
		env.OMP_PROFILE ? path.join(homeDir, ".omp", "profiles", env.OMP_PROFILE, "agent", "extensions") : undefined,
		env.PI_CODING_AGENT_DIR ? path.join(env.PI_CODING_AGENT_DIR, "extensions") : undefined,
	];
	return candidateDirectories.some(directory => {
		if (!directory) return false;
		try {
			return fs.existsSync(path.join(directory, NATIVE_HERDR_INTEGRATION_FILE));
		} catch {
			return false;
		}
	});
}

export function isAbsoluteSessionPath(file: unknown): file is string {
	return typeof file === "string" && (path.posix.isAbsolute(file) || path.win32.isAbsolute(file));
}

function relativePathWithin(base: string, target: string): string | undefined {
	const normalizedBase = path.posix.normalize(base).replace(/\/+$/, "");
	if (target === normalizedBase) return "";
	if (target.startsWith(`${normalizedBase}/`)) return target.slice(normalizedBase.length + 1);
	return undefined;
}

function joinHostPath(hostBase: string, relative: string): string {
	const hostIsWindows = /^[A-Za-z]:[\\/]/.test(hostBase) || hostBase.startsWith("\\\\");
	const joiner = hostIsWindows ? path.win32 : path.posix;
	return relative ? joiner.join(hostBase, ...relative.split("/")) : joiner.normalize(hostBase);
}

/** Map a container-side absolute path to the host path Herdr can open, following the sandbox contract. */
export function translateContainerPathToHost(containerPath: string, env: Env = process.env): string | undefined {
	if (!path.posix.isAbsolute(containerPath)) return undefined;
	const normalized = path.posix.normalize(containerPath);
	const workspaceDir = env.CLYEAN_WORKSPACE_DIR;
	const hostWorkspaceDir = env.CLYEAN_HOST_WORKSPACE_DIR;
	if (workspaceDir && hostWorkspaceDir) {
		const relative = relativePathWithin(workspaceDir, normalized);
		if (relative !== undefined) return joinHostPath(hostWorkspaceDir, relative);
	}
	if (HOST_INVISIBLE_PREFIXES.some(prefix => relativePathWithin(prefix, normalized) !== undefined)) {
		return undefined;
	}
	const hostContainerRoot = env.CLYEAN_HOST_CONTAINER_ROOT;
	if (!hostContainerRoot) return undefined;
	return joinHostPath(hostContainerRoot, normalized.slice(1));
}

export function paneTitleFor(env: Env = process.env, cwd: string = process.cwd()): string {
	const projectDir = env.CLYEAN_PROJECT_DIR || cwd;
	return path.posix.basename(projectDir.replace(/\\/g, "/")) || HERDR_DISPLAY_AGENT;
}

function parseDurationEnv(env: Env, name: string, fallback: number): number {
	const raw = env[name];
	if (!raw) return fallback;
	const parsed = Number.parseInt(raw, 10);
	if (!Number.isFinite(parsed) || parsed < 0) return fallback;
	return parsed;
}

function requestId(kind: string): string {
	return `${HERDR_SOURCE}:${kind}:${Date.now()}:${Math.random().toString(36).slice(2)}`;
}

interface HerdrTransport {
	send(request: unknown, attemptTimeouts?: readonly [number, number]): Promise<void>;
}

/** Newline-delimited JSON over the Herdr socket: bounded attempts, one retry, never throws. */
function createHerdrTransport(socketPath: string): HerdrTransport {
	const endpoint = process.platform === "win32" ? `\\\\.\\pipe\\${socketPath}` : socketPath;
	let queue: Promise<void> = Promise.resolve();

	function attempt(request: unknown, timeoutMs: number): Promise<boolean> {
		return new Promise(resolve => {
			let done = false;
			let timeout: Timer | undefined;
			const finish = (delivered: boolean) => {
				if (done) return;
				done = true;
				if (timeout) clearTimeout(timeout);
				socket.destroy();
				resolve(delivered);
			};
			const socket = net.createConnection(endpoint);
			socket.on("error", () => finish(false));
			socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
			socket.on("data", () => finish(true));
			socket.on("end", () => finish(false));
			timeout = setTimeout(() => finish(false), timeoutMs);
			timeout.unref?.();
		});
	}

	async function deliver(request: unknown, attemptTimeouts: readonly [number, number]): Promise<void> {
		if (await attempt(request, attemptTimeouts[0])) return;
		await attempt(request, attemptTimeouts[1]);
	}

	return {
		send(request, attemptTimeouts = [FIRST_ATTEMPT_TIMEOUT_MS, RETRY_ATTEMPT_TIMEOUT_MS]) {
			const run = () => deliver(request, attemptTimeouts);
			queue = queue.then(run, run);
			return queue;
		},
	};
}

function lastAssistantMessage(messages: unknown[]): any | undefined {
	for (let index = messages.length - 1; index >= 0; index -= 1) {
		const message = messages[index] as any;
		if (message?.role === "assistant") return message;
	}
	return undefined;
}

function retryableErrorMessage(event: any): string | undefined {
	const messages = Array.isArray(event?.messages) ? event.messages : [];
	const assistant = lastAssistantMessage(messages);
	if (assistant?.stopReason !== "error") return undefined;
	const errorMessage = String(assistant.errorMessage ?? "");
	if (!RETRYABLE_PROVIDER_ERROR_PATTERN.test(errorMessage)) return undefined;
	return errorMessage || "retryable provider error";
}

function askBlockedMessage(args: any): string {
	const questions = Array.isArray(args?.questions) ? args.questions : [];
	const firstQuestion = questions.find((question: any) => typeof question?.question === "string");
	return firstQuestion?.question || "waiting for user input";
}

export default function clyeanHerdrReporter(pi: ExtensionApiLike): void {
	const env: Env = process.env;
	if (!reportingEnabled(env)) return;
	if (!isUserAssistantAgent(env)) return;
	if (isNestedHarnessSession(env)) return;
	if (nativeHerdrSupportPresent(env)) return;

	const paneId = env.HERDR_PANE_ID as string;
	const transport = createHerdrTransport(env.HERDR_SOCKET_PATH as string);
	const idleDebounceMs = parseDurationEnv(env, "CLYEAN_HERDR_IDLE_DEBOUNCE_MS", 250);
	const retryGraceMs = parseDurationEnv(env, "CLYEAN_HERDR_RETRY_GRACE_MS", 2500);
	const paneTitle = paneTitleFor(env);

	// Herdr ignores a report whose seq is not above the last accepted one for
	// the same source, so the counter starts from wall-clock time to stay above
	// reports made by an earlier process that owned this pane.
	let reportSeq = Date.now() * 1000;
	let sessionPath: string | undefined;
	let sessionId: string | undefined;
	let rootSession = false;
	let restartRequested = false;
	let agentActive = false;
	let retryHoldActive = false;
	let failureBlocked = false;
	let failureMessage: string | undefined;
	let blockedCount = 0;
	let blockedMessage: string | undefined;
	let lastState: AgentState | undefined;
	let lastMessage: string | undefined;
	let idleTimer: Timer | undefined;
	let retryTimer: Timer | undefined;
	let sendInFlight = false;
	let queuedState: { state: AgentState; message?: string; seq: number } | undefined;

	function nextSeq(): number {
		reportSeq += 1;
		return reportSeq;
	}

	function updateSessionRef(ctx: HandlerContext | undefined): void {
		try {
			const file = ctx?.sessionManager?.getSessionFile?.();
			sessionPath = isAbsoluteSessionPath(file) ? translateContainerPathToHost(file, env) : undefined;
		} catch {
			sessionPath = undefined;
		}
		try {
			const id = ctx?.sessionManager?.getSessionId?.();
			sessionId = typeof id === "string" && id.length > 0 ? id : undefined;
		} catch {
			sessionId = undefined;
		}
	}

	function sessionRef(): Record<string, unknown> | undefined {
		if (sessionPath) return { agent_session_path: sessionPath };
		if (sessionId) return { agent_session_id: sessionId };
		return undefined;
	}

	function reportSession(sessionStartSource = "startup"): Promise<void> {
		const ref = sessionRef();
		if (!ref) return Promise.resolve();
		return transport.send({
			id: requestId("session"),
			method: "pane.report_agent_session",
			params: {
				pane_id: paneId,
				source: HERDR_SOURCE,
				agent: HERDR_AGENT_LABEL,
				seq: nextSeq(),
				session_start_source: sessionStartSource,
				...ref,
			},
		});
	}

	function reportMetadata(): Promise<void> {
		return transport.send({
			id: requestId("metadata"),
			method: "pane.report_metadata",
			params: {
				pane_id: paneId,
				source: HERDR_SOURCE,
				agent: HERDR_AGENT_LABEL,
				display_agent: HERDR_DISPLAY_AGENT,
				title: paneTitle,
				seq: nextSeq(),
			},
		});
	}

	function releaseAgent(): Promise<void> {
		return transport.send(
			{
				id: requestId("release"),
				method: "pane.release_agent",
				params: { pane_id: paneId, source: HERDR_SOURCE },
			},
			[RELEASE_ATTEMPT_TIMEOUT_MS, RELEASE_ATTEMPT_TIMEOUT_MS],
		);
	}

	function sendState(state: AgentState, message: string | undefined, seq: number): Promise<void> {
		return transport.send({
			id: requestId("state"),
			method: "pane.report_agent",
			params: {
				pane_id: paneId,
				source: HERDR_SOURCE,
				agent: HERDR_AGENT_LABEL,
				state,
				message,
				seq,
				...sessionRef(),
			},
		});
	}

	async function drainStateQueue(): Promise<void> {
		if (sendInFlight) return;
		sendInFlight = true;
		try {
			while (queuedState) {
				const next = queuedState;
				queuedState = undefined;
				await sendState(next.state, next.message, next.seq);
			}
		} finally {
			sendInFlight = false;
			if (queuedState) void drainStateQueue();
		}
	}

	function queueState(state: AgentState, message: string | undefined): void {
		queuedState = { state, message, seq: nextSeq() };
		if (!sendInFlight) void drainStateQueue();
	}

	function desiredState(): { state: AgentState; message?: string } {
		if (blockedCount > 0) return { state: "blocked", message: blockedMessage };
		if (failureBlocked) return { state: "blocked", message: failureMessage };
		if (agentActive || retryHoldActive) return { state: "working" };
		return { state: "idle" };
	}

	function publishState(force = false): void {
		const next = desiredState();
		if (!force && next.state === lastState && next.message === lastMessage) return;
		lastState = next.state;
		lastMessage = next.message;
		queueState(next.state, next.message);
	}

	function clearPendingTimers(): void {
		if (idleTimer) clearTimeout(idleTimer);
		if (retryTimer) clearTimeout(retryTimer);
		idleTimer = undefined;
		retryTimer = undefined;
	}

	function clearFailureState(): void {
		retryHoldActive = false;
		failureBlocked = false;
		failureMessage = undefined;
	}

	function scheduleTimer(delayMs: number, action: () => void): Timer {
		const timer = setTimeout(() => {
			try {
				action();
			} catch {
				// Reporting is best-effort; a timer callback must never take the session down.
			}
		}, delayMs);
		timer.unref?.();
		return timer;
	}

	function scheduleIdle(): void {
		clearPendingTimers();
		clearFailureState();
		idleTimer = scheduleTimer(idleDebounceMs, () => {
			idleTimer = undefined;
			publishState();
		});
	}

	function holdForRetry(message: string): void {
		clearPendingTimers();
		retryHoldActive = true;
		failureBlocked = false;
		failureMessage = message;
		publishState();
		retryTimer = scheduleTimer(retryGraceMs, () => {
			retryTimer = undefined;
			retryHoldActive = false;
			failureBlocked = true;
			publishState();
		});
	}

	function activateRootSession(ctx: HandlerContext | undefined, sessionStartSource = "startup"): boolean {
		if (ctx?.hasUI !== true) return false;
		rootSession = true;
		updateSessionRef(ctx);
		void reportSession(sessionStartSource);
		return true;
	}

	function resetSessionState(): void {
		clearPendingTimers();
		clearFailureState();
		agentActive = false;
		blockedCount = 0;
		blockedMessage = undefined;
	}

	function activateBlocked(message: string | undefined): void {
		clearPendingTimers();
		blockedCount += 1;
		blockedMessage = message;
		publishState();
	}

	function deactivateBlocked(): void {
		blockedCount = Math.max(0, blockedCount - 1);
		if (blockedCount === 0) blockedMessage = undefined;
		publishState();
	}

	pi.events.on("herdr:blocked", data => {
		if (!rootSession) return;
		if (!data?.active) {
			deactivateBlocked();
			return;
		}
		activateBlocked(typeof data.label === "string" ? data.label : undefined);
	});

	pi.on("session_start", (_event, ctx) => {
		if (ctx?.hasUI !== true) return;
		rootSession = true;
		updateSessionRef(ctx);
		void reportMetadata();
		void reportSession("startup");
		// A reload can replace this extension mid-run without another agent_start.
		agentActive = ctx?.isIdle?.() === false;
		publishState(true);
	});

	pi.on("session_switch", (event, ctx) => {
		if (!activateRootSession(ctx, event?.reason || "resume")) return;
		void reportMetadata();
		resetSessionState();
		publishState(true);
	});

	pi.on("input", event => {
		// The input hook sees raw text before slash-command dispatch, which is the
		// only place `/restart` is observable: it disposes the session (emitting
		// session_shutdown) and then exec-replaces this process with a runtime
		// that resumes the same session and re-claims the pane, so it must not
		// release Herdr authority.  /reload, /new, /resume, and /fork never
		// dispose; they emit session_switch, verified against the vendored harness.
		restartRequested = typeof event?.text === "string" && RESTART_COMMAND_PATTERN.test(event.text);
		return undefined;
	});

	pi.on("agent_start", (_event, ctx) => {
		if (!rootSession && !activateRootSession(ctx)) return;
		restartRequested = false;
		updateSessionRef(ctx);
		void reportSession();
		clearPendingTimers();
		clearFailureState();
		agentActive = true;
		publishState();
	});

	pi.on("tool_approval_requested", (event, ctx) => {
		if (!rootSession && !activateRootSession(ctx)) return;
		activateBlocked(event?.reason || `${event?.toolName || "Tool"} approval`);
	});

	pi.on("tool_approval_resolved", (_event, ctx) => {
		if (!rootSession && !activateRootSession(ctx)) return;
		deactivateBlocked();
	});

	pi.on("tool_execution_start", (event, ctx) => {
		if (event?.toolName !== "ask") return;
		if (!rootSession && !activateRootSession(ctx)) return;
		activateBlocked(askBlockedMessage(event.args));
	});

	pi.on("tool_execution_end", (event, ctx) => {
		if (event?.toolName !== "ask") return;
		if (!rootSession && !activateRootSession(ctx)) return;
		deactivateBlocked();
	});

	pi.on("agent_end", event => {
		if (!rootSession) return;
		// A duplicate or late end while a retry hold or a later turn is active
		// must not cancel the hold and publish a false idle.
		if (!agentActive) return;
		if (event?.willContinue === true) return;
		agentActive = false;
		const retryableMessage = retryableErrorMessage(event);
		if (retryableMessage) {
			holdForRetry(retryableMessage);
			return;
		}
		scheduleIdle();
	});

	pi.on("session_shutdown", async () => {
		if (!rootSession) return;
		clearPendingTimers();
		if (restartRequested) return;
		rootSession = false;
		await releaseAgent();
	});
}
