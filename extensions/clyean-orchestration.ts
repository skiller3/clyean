// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
// CLYEAN_EXTENSION_VERSION=5
// managed by clyean; upgrading clyean overwrites this file.
// add user customizations in sibling files instead of editing this one.
//
// Connects the Clyean User Assistant to the host orchestrator, which the bridge of
// the clyean process that started this container serves at the orchestrator
// socket.  The protocol is documented in docs/reference/orchestrator-protocol.md.
// The User Assistant also answers the orchestrator's requests for copies of its
// sign-ins, which other agents receive, and signs in on other agents' behalf.  It
// asks the user about relays into the sandbox, relays the harness's sign-in
// callbacks to the host's browser, edits the user's relay allowlists, and launches,
// lists, and stops server containers.

import fs from "node:fs";
import net from "node:net";
import path from "node:path";

type Env = Record<string, string | undefined>;
type Timer = ReturnType<typeof setTimeout>;
type TextContent = { type: "text"; text: string };
type ToolResult = { content: TextContent[]; details?: unknown; isError?: boolean };
type UpdateCallback = ((partialResult: ToolResult) => void) | undefined;
type HandlerContext = {
	hasUI?: boolean;
	sessionManager?: { getSessionId?: () => string | undefined };
	ui?: {
		notify?: (message: string, type?: "info" | "warning" | "error") => void;
		select?: (title: string, options: string[], options2?: { timeout?: number }) => Promise<string | undefined>;
		input?: (title: string, placeholder?: string, options?: { timeout?: number }) => Promise<string | undefined>;
		custom?: (factory: (...args: any[]) => any, options?: { overlay?: boolean }) => Promise<any>;
	};
};
type ExtensionApiLike = {
	zod: any;
	on(event: string, handler: (event: any, ctx: any) => unknown): void;
	events: { emit(channel: string, data: unknown): void };
	registerTool(definition: any): void;
	registerCommand(name: string, definition: any): void;
	sendMessage(message: unknown, options?: unknown): void;
};

export interface OrchestratorEvent {
	event: string;
	work_id?: string;
	seq?: number;
	[field: string]: unknown;
}

export interface StreamOutcome {
	result: unknown;
	terminal: OrchestratorEvent;
}

export const DEFAULT_ORCHESTRATOR_SOCKET_PATH = "/run/clyean/orchestrator.sock";
export const LEASE_METHOD = "session.lease";
export const PROMPT_TYPES = [
	"SOFTWARE_ENGINEERING_PROJECT_RESEARCH",
	"SOFTWARE_ENGINEERING_PROJECT_PLANNING",
	"SOFTWARE_ENGINEERING_PROJECT_IMPLEMENTATION",
] as const;
export const PROJECT_TYPES = ["SOFTWARE_ENGINEERING_PROJECT", "MISCELLANEOUS_PROJECT"] as const;
export const PLAN_REVIEW_WAIT_LABEL = "Change plan ready for review";
export const INFORMATION_WAIT_LABEL = "Awaiting answers for the orchestrator";
export const RELAY_APPROVAL_WAIT_LABEL = "Waiting for relay approval";
export const SIGN_IN_WAIT_LABEL = "Waiting for browser sign-in";
export const APPROVE_METHOD = "relay.approve";
export const SESSION_METHOD = "session.current";
export const APPROVAL_CHOICES = {
	project: "Allow permanently for this project",
	session: "Allow permanently for this session",
	once: "Allow once",
	deny: "Deny",
} as const;
/** How long the user has to answer a relay prompt before it is denied. */
export const APPROVAL_TIMEOUT_MS = 5 * 60_000;
/** How long a request this extension sends on the lease may take, a prompt included. */
const LEASE_REQUEST_TIMEOUT_MS = 6 * 60_000;

const USER_ASSISTANT_AGENT_ID = "user-assistant";
const TERMINAL_EVENTS = new Set(["information_requested", "completed", "failed"]);
const TRANSCRIPT_LINE_LIMIT = 400;
const DEFAULT_CONNECT_TIMEOUT_MS = 5000;
const DEFAULT_SILENCE_TIMEOUT_MS = 10 * 60 * 1000;
/** Long enough for a launch whose four relays each wait on the user, then on the server. */
const SERVER_LAUNCH_TIMEOUT_MS = 30 * 60 * 1000;

export class OrchestratorError extends Error {
	constructor(
		readonly code: string,
		message: string,
	) {
		super(message);
		this.name = "OrchestratorError";
	}
}

export function orchestratorSocketPath(env: Env = process.env): string {
	return env.CLYEAN_ORCHESTRATOR_SOCKET || DEFAULT_ORCHESTRATOR_SOCKET_PATH;
}

function parseDurationEnv(env: Env, name: string, fallback: number): number {
	const raw = env[name];
	if (!raw) return fallback;
	const parsed = Number.parseInt(raw, 10);
	if (!Number.isFinite(parsed) || parsed < 0) return fallback;
	return parsed;
}

function unreachableError(socketPath: string, cause: unknown): OrchestratorError {
	const code = (cause as { code?: string })?.code || "connection failed";
	return new OrchestratorError(
		"unreachable",
		`The Clyean host orchestrator is not reachable at ${socketPath} (${code}). The bridge of the clyean process that started this container serves it; start clyean again if that process has exited.`,
	);
}

export class OrchestratorClient {
	readonly #socketPath: string;
	readonly #connectTimeoutMs: number;
	readonly #silenceTimeoutMs: number;
	#requestCounter = 0;

	constructor(socketPath: string, options: { connectTimeoutMs?: number; silenceTimeoutMs?: number } = {}) {
		this.#socketPath = socketPath;
		this.#connectTimeoutMs = options.connectTimeoutMs ?? DEFAULT_CONNECT_TIMEOUT_MS;
		this.#silenceTimeoutMs = options.silenceTimeoutMs ?? DEFAULT_SILENCE_TIMEOUT_MS;
	}

	get socketPath(): string {
		return this.#socketPath;
	}

	/** One request, one response, no events. */
	async request(method: string, params: Record<string, unknown> = {}): Promise<unknown> {
		const outcome = await this.#exchange(method, params, undefined);
		return outcome.result;
	}

	/** One request, one response, then events until a terminal event closes the exchange. */
	stream(
		method: string,
		params: Record<string, unknown>,
		onEvent: (event: OrchestratorEvent) => void,
	): Promise<StreamOutcome> {
		return this.#exchange(method, params, onEvent) as Promise<StreamOutcome>;
	}

	#exchange(
		method: string,
		params: Record<string, unknown>,
		onEvent: ((event: OrchestratorEvent) => void) | undefined,
	): Promise<{ result: unknown; terminal?: OrchestratorEvent }> {
		const expectStream = onEvent !== undefined;
		this.#requestCounter += 1;
		const id = `${method}:${process.pid}:${this.#requestCounter}:${Date.now()}`;
		const socketPath = this.#socketPath;
		const connectTimeoutMs = this.#connectTimeoutMs;
		const silenceTimeoutMs = this.#silenceTimeoutMs;

		return new Promise((resolve, reject) => {
			let settled = false;
			let buffered = "";
			let responded = false;
			let result: unknown;
			let terminal: OrchestratorEvent | undefined;
			let lastSeq = Number.NEGATIVE_INFINITY;
			let connectTimer: Timer | undefined;
			let silenceTimer: Timer | undefined;

			const socket = net.createConnection(socketPath);

			const clearTimers = () => {
				if (connectTimer) clearTimeout(connectTimer);
				if (silenceTimer) clearTimeout(silenceTimer);
				connectTimer = undefined;
				silenceTimer = undefined;
			};
			const fail = (error: Error) => {
				if (settled) return;
				settled = true;
				clearTimers();
				socket.destroy();
				reject(error);
			};
			const succeed = () => {
				if (settled) return;
				settled = true;
				clearTimers();
				socket.destroy();
				resolve({ result, terminal });
			};
			const armSilenceTimer = () => {
				if (silenceTimer) clearTimeout(silenceTimer);
				silenceTimer = setTimeout(() => {
					fail(
						new OrchestratorError(
							"silent",
							`The Clyean host orchestrator sent nothing for ${Math.round(silenceTimeoutMs / 1000)} seconds while ${method} was in progress; the connection was abandoned.`,
						),
					);
				}, silenceTimeoutMs);
				silenceTimer.unref?.();
			};

			const handleLine = (line: string) => {
				let frame: any;
				try {
					frame = JSON.parse(line);
				} catch {
					fail(new OrchestratorError("protocol", `The orchestrator sent a line that is not JSON: ${line.slice(0, 200)}`));
					return;
				}
				if (frame && typeof frame === "object" && "id" in frame) {
					if (frame.error) {
						const code = typeof frame.error.code === "string" ? frame.error.code : "internal";
						const message = typeof frame.error.message === "string" ? frame.error.message : "unknown error";
						fail(new OrchestratorError(code, message));
						return;
					}
					responded = true;
					result = frame.result;
					if (!expectStream) succeed();
					return;
				}
				if (frame && typeof frame === "object" && typeof frame.event === "string") {
					if (!expectStream) return;
					if (typeof frame.seq === "number") {
						if (frame.seq <= lastSeq) return;
						lastSeq = frame.seq;
					}
					onEvent?.(frame as OrchestratorEvent);
					if (TERMINAL_EVENTS.has(frame.event)) {
						terminal = frame as OrchestratorEvent;
						succeed();
					}
				}
			};

			connectTimer = setTimeout(() => {
				fail(
					new OrchestratorError(
						"unreachable",
						`The Clyean host orchestrator at ${socketPath} did not accept a connection within ${connectTimeoutMs} ms.`,
					),
				);
			}, connectTimeoutMs);
			connectTimer.unref?.();

			socket.setEncoding("utf8");
			socket.on("connect", () => {
				if (connectTimer) clearTimeout(connectTimer);
				connectTimer = undefined;
				socket.write(`${JSON.stringify({ id, method, params })}\n`);
				armSilenceTimer();
			});
			socket.on("error", (error: Error) => {
				if (responded) {
					fail(new OrchestratorError("connection_lost", `The connection to the orchestrator failed: ${String(error)}`));
					return;
				}
				fail(unreachableError(socketPath, error));
			});
			socket.on("data", (chunk: string) => {
				armSilenceTimer();
				buffered += chunk;
				let newline = buffered.indexOf("\n");
				while (newline !== -1 && !settled) {
					const line = buffered.slice(0, newline).trim();
					buffered = buffered.slice(newline + 1);
					if (line.length > 0) handleLine(line);
					newline = buffered.indexOf("\n");
				}
			});
			socket.on("close", () => {
				if (settled) return;
				if (!responded) {
					fail(new OrchestratorError("connection_closed", "The orchestrator closed the connection before responding."));
					return;
				}
				if (expectStream && !terminal) {
					fail(
						new OrchestratorError(
							"connection_closed",
							"The orchestrator closed the connection before the work reached a terminal event.",
						),
					);
					return;
				}
				succeed();
			});
		});
	}
}

const LEASES = Symbol.for("clyean.orchestrator.leases");

/** The orchestrator lease of this process, shared by every load of this extension. */
export interface OrchestratorLease {
	/** Why the lease ended, once it has. */
	lost?: string;
	/** Shuts the harness down; set from the latest session context. */
	shutdown?: () => void;
	/** Answers the orchestrator's requests on the lease; set from the latest session context. */
	answer?: (method: string, params: any) => Promise<unknown>;
	/** Sends a request to the orchestrator on the lease; set once the lease is granted. */
	request?: (method: string, params: Record<string, unknown>, timeoutMs?: number) => Promise<any>;
}

/**
 * Holds one connection to the orchestrator for the life of this process.  The clyean
 * process that started this container serves it through its bridge, so the connection
 * ends exactly when that process is gone, for whatever reason, and the harness must then
 * shut down.  Once granted, the connection carries requests both ways: the orchestrator's
 * credential, relay approval, and session requests, each answered with its identifier,
 * and this extension's own sign-in relay and allowlist requests.  The socket does not
 * keep the process alive on its own.
 */
export function holdOrchestratorLease(socketPath: string): OrchestratorLease {
	const holder = globalThis as Record<symbol, Map<string, OrchestratorLease> | undefined>;
	const leases = (holder[LEASES] ??= new Map());
	const existing = leases.get(socketPath);
	if (existing) return existing;
	const lease: OrchestratorLease = {};
	leases.set(socketPath, lease);
	const pending = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void; timer: Timer }>();
	let requestCounter = 0;
	const lose = (reason: string) => {
		if (lease.lost) return;
		lease.lost = reason;
		for (const [id, waiter] of pending) {
			clearTimeout(waiter.timer);
			waiter.reject(new OrchestratorError("lease_lost", `The orchestrator lease ended: ${reason}.`));
			pending.delete(id);
		}
		lease.shutdown?.();
	};
	let buffered = "";
	let granted = false;
	const socket = net.createConnection(socketPath);
	const request = (method: string, params: Record<string, unknown>, timeoutMs = LEASE_REQUEST_TIMEOUT_MS) =>
		new Promise<any>((resolve, reject) => {
			if (lease.lost || socket.destroyed) {
				reject(new OrchestratorError("lease_lost", `The orchestrator lease ended: ${lease.lost ?? "the socket closed"}.`));
				return;
			}
			requestCounter += 1;
			const id = `ua-${process.pid}-${requestCounter}`;
			const timer = setTimeout(() => {
				pending.delete(id);
				reject(new OrchestratorError("timeout", `The orchestrator did not answer ${method} in time.`));
			}, timeoutMs);
			timer.unref?.();
			pending.set(id, { resolve, reject, timer });
			socket.write(`${JSON.stringify({ id, method, params })}\n`);
		});
	socket.unref();
	socket.setEncoding("utf8");
	socket.on("connect", () => {
		socket.write(`${JSON.stringify({ id: `lease:${process.pid}`, method: LEASE_METHOD, params: {} })}\n`);
	});
	socket.on("data", (chunk: string) => {
		buffered += chunk;
		for (let newline = buffered.indexOf("\n"); newline !== -1; newline = buffered.indexOf("\n")) {
			const line = buffered.slice(0, newline);
			buffered = buffered.slice(newline + 1);
			if (granted) {
				let frame: any;
				try {
					frame = JSON.parse(line);
				} catch {
					continue;
				}
				if (typeof frame?.method === "string") {
					void answerLeaseRequest(lease, frame).then(reply => {
						if (reply && !socket.destroyed) socket.write(`${JSON.stringify(reply)}\n`);
					});
					continue;
				}
				const waiter = typeof frame?.id === "string" ? pending.get(frame.id) : undefined;
				if (!waiter) continue;
				pending.delete(frame.id);
				clearTimeout(waiter.timer);
				if (frame.error) {
					waiter.reject(new OrchestratorError(String(frame.error.code ?? "failed"), String(frame.error.message ?? "the request failed")));
				} else {
					waiter.resolve(frame.result);
				}
				continue;
			}
			granted = true;
			let frame: any;
			try {
				frame = JSON.parse(line);
			} catch {
				frame = undefined;
			}
			if (!frame || frame.error) {
				lose(`the orchestrator refused the lease (${frame?.error?.message ?? "the answer was not JSON"})`);
				socket.destroy();
				return;
			}
			lease.request = request;
		}
	});
	socket.on("error", () => {});
	socket.on("close", () => lose("its connection to the clyean process that started this container ended"));
	return lease;
}

async function answerLeaseRequest(lease: OrchestratorLease, request: any): Promise<unknown> {
	if (typeof request?.id !== "string" || typeof request?.method !== "string") return undefined;
	if (!lease.answer) {
		return { id: request.id, error: { code: "not_ready", message: "the User Assistant's session has not started" } };
	}
	try {
		return { id: request.id, result: await lease.answer(request.method, request.params ?? {}) };
	} catch (error) {
		const code = request.method.startsWith("credentials.") ? "credentials" : "failed";
		return { id: request.id, error: { code, message: describeError(error) } };
	}
}

function bindLeaseShutdown(lease: OrchestratorLease, ctx: any): void {
	if (typeof ctx?.shutdown !== "function") return;
	lease.shutdown = () => {
		ctx.ui?.notify?.(`Clyean is shutting down: ${lease.lost}.`, "error");
		ctx.shutdown();
	};
	if (lease.lost) lease.shutdown();
}

export const RESOLVE_METHOD = "credentials.resolve";
export const VARIABLES_METHOD = "credentials.variables";
export const COPIES_METHOD = "credentials.copies";

/** How close to expiry a sign-in is refreshed before it is copied. */
const COPY_REFRESH_WINDOW_MS = 15 * 60_000;
const THINKING_LEVELS = new Set(["off", "minimal", "low", "medium", "high", "xhigh", "max", "auto"]);
const AWS_VARIABLES = [
	"AWS_ACCESS_KEY_ID",
	"AWS_SECRET_ACCESS_KEY",
	"AWS_SESSION_TOKEN",
	"AWS_REGION",
	"AWS_DEFAULT_REGION",
	"AWS_PROFILE",
	"AWS_BEARER_TOKEN_BEDROCK",
	"AWS_BEDROCK_SKIP_AUTH",
	"AWS_BEDROCK_FORCE_CACHE",
	"AWS_REGIONAL_BEDROCK_HOST",
	"AWS_ROLE_ARN",
	"AWS_WEB_IDENTITY_TOKEN_FILE",
	"AWS_CONTAINER_CREDENTIALS_FULL_URI",
	"AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
];
/** Providers whose harness credential lookup reads more than one variable. */
const PROVIDER_VARIABLES: Record<string, string[]> = {
	anthropic: ["ANTHROPIC_API_KEY", "ANTHROPIC_OAUTH_TOKEN", "ANTHROPIC_FOUNDRY_API_KEY", "CLAUDE_CODE_USE_FOUNDRY"],
	"amazon-bedrock": AWS_VARIABLES,
	"bedrock-mantle": AWS_VARIABLES,
	"google-vertex": [
		"GOOGLE_CLOUD_API_KEY",
		"GOOGLE_APPLICATION_CREDENTIALS",
		"GOOGLE_CLOUD_PROJECT",
		"GCP_PROJECT",
		"GCLOUD_PROJECT",
		"GOOGLE_VERTEX_LOCATION",
		"GOOGLE_CLOUD_LOCATION",
		"VERTEX_LOCATION",
	],
	commandcode: ["COMMAND_CODE_API_KEY", "COMMANDCODE_API_KEY"],
	"charm-hyper": ["CHARM_HYPER_API_KEY", "HYPER_API_KEY"],
};

type StoredRow = { id: number; credential: any };
/** The part of the harness's login store the authority uses. */
export interface AuthorityStore {
	listStoredCredentials(provider: string): StoredRow[];
	hasAuth(provider: string): boolean;
	set(provider: string, credential: any): Promise<void>;
	refreshStoredOAuthCredential(provider: string, options: any): Promise<unknown>;
}
export interface ModelQuery {
	resolve(spec: string): { provider: string; id: string } | undefined;
	current(): { provider: string; id: string } | undefined;
}
export interface McpSignInCallbacks {
	onAuth(info: { url: string }): void;
	onManualCodeInput(): Promise<string>;
}
/** The harness facilities the credential authority needs, injectable for tests. */
export interface AuthorityHarness {
	envApiKeyName(provider: string): string | undefined;
	refreshModelCredential(provider: string, credential: any, signal?: AbortSignal): Promise<any>;
	refreshMcpCredential(credential: any, serverUrl: string, signal?: AbortSignal): Promise<any>;
	isDefinitiveOAuthFailure(message: string): boolean;
	mcpCredentialId(serverUrl: string, profile: string): string;
	signInToMcpServer(serverUrl: string, server: any, callbacks: McpSignInCallbacks): Promise<any>;
}

/** Splits a trailing thinking level (`:high`) off a model pattern. */
export function splitThinkingLevel(pattern: string): { base: string; level?: string } {
	const colon = pattern.lastIndexOf(":");
	const level = colon === -1 ? undefined : pattern.slice(colon + 1);
	return level && THINKING_LEVELS.has(level) ? { base: pattern.slice(0, colon), level } : { base: pattern };
}

/**
 * Maps model patterns to providers with the harness's own resolver, falling back to the
 * whole catalog for providers the User Assistant has not signed in to, and reports the
 * User Assistant's current model.
 */
export function resolveModels(models: ModelQuery, catalog: Array<{ provider: string; id: string }>, patterns: string[]) {
	const providers = new Set(catalog.map(model => model.provider));
	const resolved: Record<string, { provider: string; model: string | null } | null> = {};
	for (const pattern of patterns) {
		const { base, level } = splitThinkingLevel(pattern);
		const model = models.resolve(pattern) ?? catalog.find(entry => `${entry.provider}/${entry.id}` === base);
		if (model) {
			resolved[pattern] = { provider: model.provider, model: `${model.provider}/${model.id}${level ? `:${level}` : ""}` };
			continue;
		}
		const provider = base.includes("/") ? base.slice(0, base.indexOf("/")) : "";
		resolved[pattern] = providers.has(provider) ? { provider, model: null } : null;
	}
	const current = models.current();
	return {
		type: "models_resolved",
		models: resolved,
		current: current ? { provider: current.provider, model: `${current.provider}/${current.id}` } : null,
	};
}

/** The environment variables the harness reads for each provider. */
export function providerVariables(harness: AuthorityHarness, providers: string[]) {
	const variables: Record<string, string[]> = {};
	for (const provider of providers) {
		const single = harness.envApiKeyName(provider);
		variables[provider] = PROVIDER_VARIABLES[provider] ?? (single ? [single] : []);
	}
	return { type: "credential_variables", variables };
}

/** A copy of a stored credential: usable, but without what refreshes it. */
function copyOf(credential: any): any {
	if (credential?.type === "api_key") return credential;
	if (credential?.type !== "oauth") return undefined;
	if (credential.expires && credential.expires <= Date.now()) return undefined;
	const { clientSecret: _clientSecret, ...copy } = credential;
	return { ...copy, refresh: "" };
}

/** Refreshes every OAuth credential of `provider` that expires within the copy window. */
async function refreshBeforeCopying(
	store: AuthorityStore,
	harness: AuthorityHarness,
	provider: string,
	refresh: (credential: any, signal?: AbortSignal) => Promise<any>,
	canRefresh?: (credential: any) => boolean,
): Promise<void> {
	for (const row of store.listStoredCredentials(provider)) {
		const credential = row.credential;
		if (credential?.type !== "oauth" || !credential.expires) continue;
		if (Date.now() + COPY_REFRESH_WINDOW_MS < credential.expires) continue;
		try {
			await store.refreshStoredOAuthCredential(provider, {
				credentialId: row.id,
				observedCredential: credential,
				credentialFromRow: (stored: any) => stored,
				refreshSkewMs: COPY_REFRESH_WINDOW_MS,
				canRefresh,
				refresh,
				mergeRefreshedCredential: (current: any, refreshed: any) => ({ ...current, ...refreshed }),
				isDefinitiveFailure: (error: unknown) => harness.isDefinitiveOAuthFailure(describeError(error)),
				disabledCause: (error: unknown) => `oauth refresh failed: ${describeError(error)}`,
				keepCredentialOnRefreshFailure: true,
			});
		} catch {
			// A credential that could not be refreshed is copied while it remains valid.
		}
	}
}

/**
 * Copies of this User Assistant's credentials for `providers` and `mcp_servers`, made for
 * `agent`: refreshed first when they expire soon, without refresh tokens or client
 * secrets, and with MCP credentials keyed to `agent`'s profile.
 */
export async function copyCredentials(
	store: AuthorityStore,
	harness: AuthorityHarness,
	params: { agent: string; providers: string[]; mcp_servers: string[] },
) {
	const copies = {
		type: "credential_copies",
		providers: {} as Record<string, unknown[]>,
		mcp: {} as Record<string, unknown>,
		unavailable: [] as string[],
	};
	for (const provider of params.providers) {
		await refreshBeforeCopying(store, harness, provider, (credential, signal) =>
			harness.refreshModelCredential(provider, credential, signal),
		);
		const rows = store.listStoredCredentials(provider).map(row => copyOf(row.credential)).filter(Boolean);
		if (rows.length > 0) copies.providers[provider] = rows;
		if (!store.hasAuth(provider)) copies.unavailable.push(provider);
	}
	for (const serverUrl of params.mcp_servers) {
		const ownId = harness.mcpCredentialId(serverUrl, USER_ASSISTANT_AGENT_ID);
		await refreshBeforeCopying(
			store,
			harness,
			ownId,
			(credential, signal) => harness.refreshMcpCredential(credential, serverUrl, signal),
			credential => Boolean(credential.refresh && credential.tokenUrl),
		);
		const row = store.listStoredCredentials(ownId).find(entry => entry.credential?.type === "oauth");
		const copy = row && copyOf(row.credential);
		if (copy) copies.mcp[harness.mcpCredentialId(serverUrl, params.agent)] = copy;
	}
	return copies;
}

function bindLeaseAuthority(
	lease: OrchestratorLease,
	ctx: any,
	harness: () => Promise<AuthorityHarness>,
	approvals: RelayApprovals,
): void {
	const registry = ctx?.modelRegistry;
	const credentialsReady = Boolean(registry?.authStorage && ctx?.models);
	lease.answer = async (method, params) => {
		if (method === SESSION_METHOD) return { session_id: sessionIdOf(ctx) ?? null };
		if (method === APPROVE_METHOD) return approvals.approve(ctx, params);
		if (!credentialsReady) throw new Error("the User Assistant's session has not started");
		switch (method) {
			case RESOLVE_METHOD:
				return resolveModels(ctx.models, registry.getAll(), stringList(params.patterns));
			case VARIABLES_METHOD:
				return providerVariables(await harness(), stringList(params.providers));
			case COPIES_METHOD:
				return copyCredentials(registry.authStorage, await harness(), {
					agent: String(params.agent ?? ""),
					providers: stringList(params.providers),
					mcp_servers: stringList(params.mcp_servers),
				});
			default:
				throw new Error(`unknown method ${method}`);
		}
	};
}

function deepMerge(base: unknown, overlay: unknown): unknown {
	if (!isRecord(base) || !isRecord(overlay)) return overlay;
	const merged: Record<string, unknown> = { ...base };
	for (const [key, value] of Object.entries(overlay)) {
		if (value === null) delete merged[key];
		else merged[key] = key in merged ? deepMerge(merged[key], value) : value;
	}
	return merged;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readJsonIfPresent(file: string): unknown {
	try {
		return JSON.parse(fs.readFileSync(file, "utf8"));
	} catch (error: any) {
		if (error?.code === "ENOENT") return undefined;
		throw new Error(`${file} is not valid JSON: ${describeError(error)}`);
	}
}

/** An agent's MCP servers: `.clyean/agents/<AGENT_NAME>.mcp.json` with its local companion on top. */
export function agentMcpServers(projectDir: string, agent: string): Record<string, any> {
	const stem = agent.toUpperCase().replaceAll("-", "_");
	const tracked = path.join(projectDir, ".clyean", "agents", `${stem}.mcp.json`);
	const local = path.join(projectDir, ".clyean", "agents", `${stem}.mcp.local.json`);
	const merged = deepMerge(readJsonIfPresent(tracked) ?? {}, readJsonIfPresent(local) ?? {});
	const servers = isRecord(merged) ? merged.mcpServers : undefined;
	return isRecord(servers) ? servers : {};
}

/**
 * Signs in to an MCP server configured for another agent, through the harness's MCP
 * OAuth flow, and records the credential in this User Assistant's login store, from which
 * copies reach that agent.
 */
export async function signInOnBehalf(
	request: { agent: string; server: string; projectDir: string },
	store: Pick<AuthorityStore, "set">,
	harness: AuthorityHarness,
	callbacks: McpSignInCallbacks,
): Promise<string> {
	if (!/^[a-z][a-z-]*$/.test(request.agent)) throw new Error(`${request.agent} is not an agent identifier`);
	const server = agentMcpServers(request.projectDir, request.agent)[request.server];
	if (!server) {
		throw new Error(`the ${request.agent} agent has no MCP server named ${request.server} in .clyean/agents`);
	}
	if (typeof server.url !== "string") {
		throw new Error(`${request.server} is not a remote MCP server, so it has no sign-in`);
	}
	const credential = await harness.signInToMcpServer(server.url, server, callbacks);
	await store.set(harness.mcpCredentialId(server.url, USER_ASSISTANT_AGENT_ID), credential);
	return server.url;
}

async function loadAuthorityHarness(): Promise<AuthorityHarness> {
	const [ai, oauth, flow, discovery, mcp] = await Promise.all([
		import("@oh-my-pi/pi-ai"),
		import("@oh-my-pi/pi-ai/oauth"),
		import("@oh-my-pi/pi-coding-agent/mcp/oauth-flow"),
		import("@oh-my-pi/pi-coding-agent/mcp/oauth-discovery"),
		import("@oh-my-pi/pi-coding-agent/mcp/oauth-credentials"),
	]);
	return {
		envApiKeyName: provider => ai.getEnvApiKeyName(provider),
		refreshModelCredential: (provider, credential, signal) => {
			const custom = oauth.getOAuthProvider(provider);
			return custom?.refreshToken
				? custom.refreshToken(credential, signal)
				: oauth.refreshOAuthToken(provider, credential, signal);
		},
		refreshMcpCredential: (credential, serverUrl, signal) =>
			mcp.refreshManagedMcpOAuthCredential(credential, { serverUrl, signal }),
		isDefinitiveOAuthFailure: message => ai.isDefinitiveOAuthFailure(message),
		mcpCredentialId: (serverUrl, profile) => flow.mcpOAuthCredentialId(serverUrl, profile),
		async signInToMcpServer(serverUrl, server, callbacks) {
			const endpoints = await discovery.discoverOAuthEndpoints(serverUrl);
			if (!endpoints) throw new Error(`${serverUrl} advertises no OAuth sign-in`);
			const hints = server.oauth ?? {};
			const signIn = new flow.MCPOAuthFlow(
				{
					authorizationUrl: endpoints.authorizationUrl,
					tokenUrl: endpoints.tokenUrl,
					registrationUrl: endpoints.registrationUrl,
					issuerUrl: endpoints.issuerUrl,
					clientId: hints.clientId ?? endpoints.clientId,
					clientSecret: hints.clientSecret,
					scopes: endpoints.scopes,
					resource: endpoints.resource,
					prompt: hints.prompt,
					redirectUri: hints.redirectUri,
					callbackPort: hints.callbackPort,
					callbackPath: hints.callbackPath,
				},
				{
					onAuth: (info: { url: string }) => callbacks.onAuth(info),
					onProgress: () => {},
					onManualCodeInput: () => callbacks.onManualCodeInput(),
					signal: AbortSignal.timeout(SIGN_IN_TIMEOUT_MS),
				},
			);
			const credentials = await signIn.login();
			return {
				type: "oauth",
				...credentials,
				tokenUrl: endpoints.tokenUrl,
				clientId: signIn.resolvedClientId ?? hints.clientId ?? endpoints.clientId,
				clientSecret: signIn.registeredClientSecret ?? hints.clientSecret,
				resource: signIn.resource,
				authorizationUrl: signIn.authorizationUrl,
			};
		},
	};
}

const SIGN_IN_TIMEOUT_MS = 5 * 60_000;

class BoundedTranscript {
	readonly #lines: string[] = [];
	#dropped = 0;

	constructor(private readonly limit: number) {}

	append(line: string): void {
		this.#lines.push(line);
		if (this.#lines.length > this.limit) {
			this.#lines.shift();
			this.#dropped += 1;
		}
	}

	get isEmpty(): boolean {
		return this.#lines.length === 0 && this.#dropped === 0;
	}

	text(): string {
		const prefix = this.#dropped > 0 ? [`(${this.#dropped} earlier lines omitted)`] : [];
		return [...prefix, ...this.#lines].join("\n");
	}
}

function formatEventLine(event: OrchestratorEvent): string {
	const agent = typeof event.agent === "string" ? event.agent : "orchestrator";
	const phase = typeof event.phase === "string" ? event.phase : "";
	const text = typeof event.text === "string" ? event.text : "";
	const label = phase ? `${agent}/${phase}` : agent;
	return `[${label}] ${text}`.trimEnd();
}

function textResult(text: string, details?: unknown, isError = false): ToolResult {
	const result: ToolResult = { content: [{ type: "text", text }], details };
	if (isError) result.isError = true;
	return result;
}

function describeError(error: unknown): string {
	if (error instanceof OrchestratorError) return error.message;
	return error instanceof Error ? error.message : String(error);
}

function stringList(value: unknown): string[] {
	return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

interface IncompleteWork {
	work_id: string;
	prompt_type?: string;
	session_id?: string;
	phase?: string;
	started_at?: string;
}

function incompleteWorkOf(status: any): IncompleteWork[] {
	const entries = Array.isArray(status?.incomplete_work) ? status.incomplete_work : [];
	return entries.filter((entry: any) => entry && typeof entry.work_id === "string");
}

export function renderProjectStatus(status: any): string {
	const lines = [
		`Project scaffolded: ${status?.scaffolded ? "yes" : "no"}`,
		`Project type: ${status?.project_type ?? "not determined yet"}`,
		`Project locked: ${status?.locked ? "yes" : "no"}`,
	];
	const incomplete = incompleteWorkOf(status);
	lines.push(`Incomplete work: ${incomplete.length}`);
	for (const work of incomplete) {
		const details = [work.prompt_type, work.phase ? `phase ${work.phase}` : undefined, work.started_at ? `started ${work.started_at}` : undefined, work.session_id ? `session ${work.session_id}` : undefined]
			.filter(Boolean)
			.join(", ");
		lines.push(`  - ${work.work_id}${details ? ` (${details})` : ""}`);
	}
	return lines.join("\n");
}

function renderInformationRequest(event: OrchestratorEvent, workId: string | undefined): string {
	const questions = stringList(event.questions);
	const requestId = typeof event.request_id === "string" ? event.request_id : "";
	const context = typeof event.context === "string" && event.context.trim() ? event.context.trim() : undefined;
	const lines = [
		`The orchestrator needs more information before work ${workId ?? "(unknown)"} can continue.`,
		"Ask the user the following questions with your `ask` tool, then call `clyean_provide_information` with",
		`work_id "${workId ?? ""}", request_id "${requestId}", and one answer per question in the same order.`,
		"",
		...questions.map((question, index) => `${index + 1}. ${question}`),
	];
	if (context) lines.push("", `Context from the orchestrator: ${context}`);
	return lines.join("\n");
}

function renderCompletion(event: OrchestratorEvent, workId: string | undefined, transcript: BoundedTranscript): string {
	const summary = typeof event.summary === "string" ? event.summary : "The work completed.";
	const artifacts = stringList(event.artifacts);
	const plan = typeof event.plan === "string" ? event.plan : undefined;
	const lines = [`Work ${workId ?? "(unknown)"} completed.`, "", summary];
	if (plan) lines.push("", `Change plan: ${plan}`);
	if (artifacts.length > 0) lines.push("", "Artifacts:", ...artifacts.map(artifact => `  - ${artifact}`));
	if (!transcript.isEmpty) lines.push("", "Progress log:", transcript.text());
	return lines.join("\n");
}

/** Tracks the single orchestration-level wait surfaced to Herdr through the custom event bus. */
class HerdrWaitSignal {
	#activeLabel: string | undefined;

	constructor(private readonly emit: (data: unknown) => void) {}

	begin(label: string): void {
		this.clear();
		this.#activeLabel = label;
		this.emit({ active: true, label });
	}

	clear(): void {
		if (this.#activeLabel === undefined) return;
		const label = this.#activeLabel;
		this.#activeLabel = undefined;
		this.emit({ active: false, label });
	}
}

/** The User Assistant's session id, when the harness has one. */
function sessionIdOf(ctx: HandlerContext | undefined): string | undefined {
	try {
		const id = ctx?.sessionManager?.getSessionId?.();
		return typeof id === "string" && id.length > 0 ? id : undefined;
	} catch {
		return undefined;
	}
}

/** `127.0.0.1:3000` and `[::1]:3000` style addresses of a relay's host side. */
export function relayEndpoints(relay: any): string[] {
	const port = relay?.host_port;
	return stringList(relay?.host_addresses).map(address => (address.includes(":") ? `[${address}]:${port}` : `${address}:${port}`));
}

/** The lines and choices of a relay prompt, from the facts the orchestrator sends. */
export function renderApprovalPrompt(facts: any): { lines: string[]; options: string[] } {
	const requester = facts?.requester ?? {};
	const relay = facts?.relay ?? {};
	const server = facts?.server;
	const lines: string[] = [];
	if (requester.kind === "agent") {
		lines.push(`Relay request from the ${requester.display ?? requester.agent ?? "sub-agent"}`);
	} else if (requester.kind === "sign_in") {
		lines.push(`The User Assistant's sign-in to ${requester.provider ?? "a provider"} needs a relay`);
	} else {
		lines.push("Relay request from a process in the User Assistant's container");
		if (requester.command) lines.push(`  Process ${requester.pid ?? "?"}: ${requester.command}   (as it reports itself)`);
	}
	if (server) {
		const lifetime = server.persistent ? "kept running after Clyean exits" : "stopped when Clyean exits";
		lines.push(`  Server "${server.name}" in a container of its own, ${lifetime}`);
		lines.push(`  Command: ${server.command}   (in ${server.cwd})`);
	}
	const protocol = String(relay.protocol ?? "tcp").toUpperCase();
	const where = server ? "in the server" : "in the User Assistant's container";
	lines.push(`  ${protocol}  ${relayEndpoints(relay).join(" and ")} on this computer  ->  port ${relay.guest_port} ${where}`);
	if (typeof facts?.reason === "string" && facts.reason.trim()) lines.push(`  Reason given: "${facts.reason.trim()}"`);
	if (typeof facts?.warning === "string") lines.push(`  Warning: ${facts.warning}`);
	if (typeof facts?.file_problem === "string") lines.push(`  Nothing can be allowed permanently: ${facts.file_problem}`);
	const options: string[] = [];
	if (facts?.permanent_available !== false) {
		options.push(APPROVAL_CHOICES.project);
		if (facts?.session_available) options.push(APPROVAL_CHOICES.session);
	}
	options.push(APPROVAL_CHOICES.once, APPROVAL_CHOICES.deny);
	return { lines, options };
}

function decisionFor(choice: string | undefined): { decision: string; reason?: string } {
	switch (choice) {
		case APPROVAL_CHOICES.project:
			return { decision: "project" };
		case APPROVAL_CHOICES.session:
			return { decision: "session" };
		case APPROVAL_CHOICES.once:
			return { decision: "once" };
		case TIMED_OUT:
			return { decision: "deny", reason: "timed_out" };
		default:
			return { decision: "deny", reason: "denied" };
	}
}

const TIMED_OUT = "\u0000timed out";

/**
 * A prompt drawn over the transcript rather than in the editor's place, so a login dialog
 * waiting underneath stays where it is.  Arrow keys or digits choose, Enter confirms, and
 * Escape denies.
 */
export function approvalOverlay(
	lines: string[],
	options: string[],
	done: (choice: string) => void,
	requestRender: () => void,
	timeoutMs: number,
	accent: (text: string) => string = text => text,
) {
	let selected = 0;
	let finished = false;
	const finish = (choice: string) => {
		if (finished) return;
		finished = true;
		clearTimeout(timer);
		done(choice);
	};
	const timer = setTimeout(() => finish(TIMED_OUT), timeoutMs);
	timer.unref?.();
	const fit = (text: string, width: number) => (text.length > width ? `${text.slice(0, Math.max(0, width - 1))}…` : text);
	return {
		render(width: number): string[] {
			const rows = lines.map(line => fit(line, width));
			rows.push("");
			options.forEach((option, index) => {
				const marker = index === selected ? "> " : "  ";
				const row = fit(`${marker}${index + 1}. ${option}`, width);
				rows.push(index === selected ? accent(row) : row);
			});
			return rows;
		},
		handleInput(data: string): void {
			if (data === "\u001b[A" || data === "\u001bOA" || data === "k") selected = (selected + options.length - 1) % options.length;
			else if (data === "\u001b[B" || data === "\u001bOB" || data === "j") selected = (selected + 1) % options.length;
			else if (data === "\r" || data === "\n") finish(options[selected]);
			else if (data === "\u001b") finish(APPROVAL_CHOICES.deny);
			else if (/^[1-9]$/.test(data) && Number(data) <= options.length) finish(options[Number(data) - 1]);
			else return;
			requestRender();
		},
		invalidate(): void {},
		dispose(): void {
			clearTimeout(timer);
		},
	};
}

/** Asks the user about relays, one prompt at a time, and signals Herdr while one is open. */
export class RelayApprovals {
	#queue: Promise<unknown> = Promise.resolve();

	constructor(
		private readonly wait: HerdrWaitSignal,
		private readonly timeoutMs: number = APPROVAL_TIMEOUT_MS,
	) {}

	/** Prompts wait their turn, and each has the time the host allows from its arrival, not from its display. */
	approve(ctx: any, facts: any): Promise<{ decision: string; reason?: string }> {
		const deadline = Date.now() + this.timeoutMs;
		const next = this.#queue.then(() => this.#ask(ctx, facts, deadline));
		this.#queue = next.catch(() => undefined);
		return next;
	}

	async #ask(ctx: any, facts: any, deadline: number): Promise<{ decision: string; reason?: string }> {
		if (ctx?.hasUI !== true || !ctx?.ui) return { decision: "deny", reason: "no_user_interface" };
		const remainingMs = deadline - Date.now();
		if (remainingMs <= 0) return decisionFor(TIMED_OUT);
		const { lines, options } = renderApprovalPrompt(facts);
		this.wait.begin(RELAY_APPROVAL_WAIT_LABEL);
		try {
			return decisionFor(await this.#show(ctx, lines, options, remainingMs));
		} catch {
			return { decision: "deny", reason: "no_user_interface" };
		} finally {
			this.wait.clear();
		}
	}

	async #show(ctx: any, lines: string[], options: string[], timeoutMs: number): Promise<string | undefined> {
		if (typeof ctx.ui.custom === "function") {
			return ctx.ui.custom(
				(tui: any, theme: any, _keybindings: unknown, done: (choice: string) => void) =>
					approvalOverlay(
						lines,
						options,
						done,
						() => tui?.requestRender?.(),
						timeoutMs,
						text => (typeof theme?.fg === "function" ? theme.fg("accent", text) : text),
					),
				{ overlay: true },
			);
		}
		if (typeof ctx.ui.select !== "function") return undefined;
		const started = Date.now();
		const choice = await ctx.ui.select(lines.join("\n"), options, { timeout: timeoutMs });
		return choice ?? (Date.now() - started >= timeoutMs ? TIMED_OUT : undefined);
	}
}

/** The loopback and wildcard TCP ports listening in a `/proc/net/tcp` or `tcp6` table. */
export function listeningPorts(table: string): Set<number> {
	const ports = new Set<number>();
	for (const line of table.split("\n").slice(1)) {
		const fields = line.trim().split(/\s+/);
		if (fields.length < 4 || fields[3] !== "0A") continue;
		const [address, port] = fields[1].split(":");
		if (!address || !port) continue;
		const loopbackOrWildcard =
			/^0+$/.test(address) ||
			/7F$/i.test(address) ||
			address.toUpperCase() === "00000000000000000000000001000000" ||
			address.toUpperCase() === "0000000000000000FFFF00000100007F";
		if (loopbackOrWildcard) ports.add(Number.parseInt(port, 16));
	}
	return ports;
}

/** What the sign-in watcher needs from its surroundings, injectable for tests. */
export interface SignInSurroundings {
	/** The container's TCP socket tables. */
	socketTables(): string[];
	/** The provider a port's callback server signs in to, or nothing for other servers. */
	fingerprint(port: number): Promise<{ provider: string } | undefined>;
	pollMs: number;
	windowMs: number;
	closeDelayMs: number;
}

export const SIGN_IN_SURROUNDINGS: SignInSurroundings = {
	socketTables: () => ["/proc/net/tcp", "/proc/net/tcp6"].map(file => {
		try {
			return fs.readFileSync(file, "utf8");
		} catch {
			return "";
		}
	}),
	fingerprint: fingerprintCallbackServer,
	pollMs: 250,
	windowMs: 10 * 60_000,
	closeDelayMs: 2_000,
};

/**
 * The harness's callback server answers `/launch` with 302 to the authorization page
 * while a sign-in is pending, and with 503 otherwise; the redirect names the provider.
 */
async function fingerprintCallbackServer(port: number): Promise<{ provider: string } | undefined> {
	try {
		const response = await fetch(`http://127.0.0.1:${port}/launch`, { redirect: "manual", signal: AbortSignal.timeout(1_500) });
		if (response.status === 503) return { provider: "a provider" };
		if (response.status !== 302) return undefined;
		try {
			return { provider: new URL(response.headers.get("location") ?? "").hostname || "a provider" };
		} catch {
			return { provider: "a provider" };
		}
	} catch {
		return undefined;
	}
}

/**
 * Watches for the harness's sign-in callback server during a sign-in window and asks the
 * host, over the lease, to relay it to the host's loopback, where the browser returns.
 */
export class SignInWatcher {
	#baseline = new Set<number>();
	#ignored = new Set<number>();
	#relays = new Map<number, { relayId: string; missingSince?: number }>();
	#asking = new Set<number>();
	#deadline = 0;
	#timer: Timer | undefined;

	constructor(
		private readonly surroundings: SignInSurroundings,
		private readonly lease: () => OrchestratorLease | undefined,
		private readonly notify: (message: string, type: "info" | "warning") => void,
		private readonly wait: HerdrWaitSignal,
	) {}

	get watching(): boolean {
		return this.#timer !== undefined;
	}

	/** Opens a sign-in window, or extends the open one. */
	open(): void {
		this.#deadline = Date.now() + this.surroundings.windowMs;
		if (this.#timer) return;
		this.#baseline = this.#listening();
		this.#ignored.clear();
		this.#timer = setInterval(() => void this.#tick(), this.surroundings.pollMs);
		this.#timer.unref?.();
	}

	/** Ends the window and closes its relays. */
	close(): void {
		if (this.#timer) clearInterval(this.#timer);
		this.#timer = undefined;
		for (const port of [...this.#relays.keys()]) this.#closeRelay(port);
	}

	#listening(): Set<number> {
		const ports = new Set<number>();
		for (const table of this.surroundings.socketTables()) for (const port of listeningPorts(table)) ports.add(port);
		return ports;
	}

	async #tick(): Promise<void> {
		if (Date.now() > this.#deadline) {
			this.close();
			return;
		}
		const listening = this.#listening();
		for (const [port, relay] of this.#relays) {
			if (listening.has(port)) relay.missingSince = undefined;
			else if (relay.missingSince === undefined) relay.missingSince = Date.now();
			else if (Date.now() - relay.missingSince >= this.surroundings.closeDelayMs) this.#closeRelay(port);
		}
		for (const port of listening) {
			if (this.#baseline.has(port) || this.#ignored.has(port) || this.#relays.has(port) || this.#asking.has(port)) continue;
			this.#asking.add(port);
			void this.#consider(port).finally(() => this.#asking.delete(port));
		}
	}

	async #consider(port: number): Promise<void> {
		const callback = await this.surroundings.fingerprint(port);
		if (!callback) {
			this.#ignored.add(port);
			return;
		}
		const lease = this.lease();
		if (!lease?.request) {
			this.#ignored.add(port);
			return;
		}
		try {
			const opened = await lease.request("relay.open", {
				protocol: "tcp",
				host_port: port,
				guest_port: port,
				purpose: "sign-in",
				provider: callback.provider,
			});
			this.#relays.set(port, { relayId: String(opened?.relay_id ?? "") });
			this.wait.begin(SIGN_IN_WAIT_LABEL);
			this.notify(`Browser sign-in relayed through http://localhost:${port}.`, "info");
		} catch (error) {
			this.#ignored.add(port);
			this.notify(
				`The sign-in callback on port ${port} could not be relayed (${describeError(error)}); paste the address your browser ends on into the login dialog.`,
				"warning",
			);
		}
	}

	#closeRelay(port: number): void {
		const relay = this.#relays.get(port);
		if (!relay) return;
		this.#relays.delete(port);
		void this.lease()?.request?.("relay.close", { relay_id: relay.relayId }).catch(() => undefined);
		if (this.#relays.size === 0) this.wait.clear();
	}
}

/** Whether interactive input starts a sign-in, whose callback may need a relay. */
export function startsSignIn(text: string): boolean {
	return /^\/(login|mcp|model)\b/.test(text) || /^\/clyean\s+sign-in\b/.test(text);
}

function renderRelayOpened(result: any): string {
	const approval = result?.approval === "once" ? "allowed once" : `allowed for the ${result?.approval}`;
	return `Relay ${result?.relay_id} open: ${String(result?.protocol ?? "").toUpperCase()} ${stringList(result?.endpoints).join(", ")} -> port ${result?.guest_port} in this container (${approval}).`;
}

export function renderServerList(listing: any): string {
	const servers: any[] = Array.isArray(listing?.servers) ? listing.servers : [];
	if (servers.length === 0) return "No servers are running for this project.";
	return servers.map(server => String(server.summary ?? server.name)).join("\n");
}

/** The `server.launch` parameters among the `clyean_server` tool's. */
export function serverLaunchParams(params: any): Record<string, unknown> {
	if (!params?.name || !params?.command) throw new Error("launch needs a name and a command");
	if (!Array.isArray(params.ports) || params.ports.length === 0) {
		throw new Error("launch needs the ports the server listens on, such as [{\"protocol\": \"tcp\", \"port\": 5173}]");
	}
	const request: Record<string, unknown> = { name: params.name, command: params.command, ports: params.ports };
	for (const key of ["cwd", "persistent", "relays", "ready_timeout_seconds", "reason"]) {
		if (params[key] !== undefined) request[key] = params[key];
	}
	return request;
}

export function renderRelayList(relays: any[]): string {
	if (relays.length === 0) return "No relays are open.";
	return relays
		.map(relay => `${relay.relay_id}: ${String(relay.protocol).toUpperCase()} ${stringList(relay.endpoints).join(", ")} -> port ${relay.guest_port} (${relay.approval}, ${relay.connections ?? 0} open connections)`)
		.join("\n");
}

export function renderAllowlist(listing: any): string {
	const section = (title: string, entries: any[]) => [
		title,
		...(entries.length === 0 ? ["  (none)"] : entries.map((entry, index) => `  ${index + 1}. ${entry.text}`)),
	];
	const lines = [
		`Relays ${listing?.enabled === false ? "are turned off" : "are on"} (${listing?.file ?? "relay file"})`,
		...section("Open relays:", []).slice(0, 1),
		...(Array.isArray(listing?.relays) && listing.relays.length > 0 ? renderRelayList(listing.relays).split("\n").map(line => `  ${line}`) : ["  (none)"]),
		...section(`Allowed for this project (${listing?.project_dir ?? ""}):`, Array.isArray(listing?.project) ? listing.project : []),
		...section("Allowed for this session:", Array.isArray(listing?.session) ? listing.session : []),
	];
	if (typeof listing?.file_problem === "string") lines.push(`The relay file cannot be read: ${listing.file_problem}`);
	return lines.join("\n");
}

/** Reads `/clyean relays allow <scope> <protocols> <host ports> <guest ports> <addresses>`. */
export function parseAllowCommand(words: string[]): { scope: string; entry: Record<string, unknown> } {
	const [scope, protocols, hostPorts, guestPorts, addresses] = words;
	if (!scope || !protocols || !hostPorts || !guestPorts || !addresses || words.length > 5 || !["project", "session"].includes(scope)) {
		throw new Error("usage: /clyean relays allow <project|session> <protocols> <host ports> <guest ports> <addresses>");
	}
	return {
		scope,
		entry: {
			protocols: protocols.split(",").map(protocol => protocol.trim().toLowerCase()),
			hostPorts,
			guestPorts,
			hostAddresses: addresses.split(",").map(address => address.trim()),
		},
	};
}

const ADDRESS_PRESETS: Record<string, string[]> = {
	"Loopback (127.0.0.1 and ::1)": ["127.0.0.1", "::1"],
	"Every IPv4 address (0.0.0.0)": ["0.0.0.0"],
	"Every IPv6 address (::)": ["::"],
	"Every IPv4 and IPv6 address": ["0.0.0.0", "::"],
};

export default function clyeanOrchestration(
	pi: ExtensionApiLike,
	authorityHarness: () => Promise<AuthorityHarness> = loadAuthorityHarness,
	signInSurroundings: SignInSurroundings = SIGN_IN_SURROUNDINGS,
): void {
	const env: Env = process.env;
	if (env.CLYEAN_AGENT && env.CLYEAN_AGENT !== USER_ASSISTANT_AGENT_ID) return;

	const connectTimeoutMs = parseDurationEnv(env, "CLYEAN_ORCHESTRATOR_CONNECT_TIMEOUT_MS", DEFAULT_CONNECT_TIMEOUT_MS);
	const client = new OrchestratorClient(orchestratorSocketPath(env), {
		connectTimeoutMs,
		silenceTimeoutMs: parseDurationEnv(env, "CLYEAN_ORCHESTRATOR_SILENCE_TIMEOUT_MS", DEFAULT_SILENCE_TIMEOUT_MS),
	});
	const serverClient = new OrchestratorClient(orchestratorSocketPath(env), { connectTimeoutMs, silenceTimeoutMs: SERVER_LAUNCH_TIMEOUT_MS });
	const lease = env.CLYEAN_ORCHESTRATOR_LEASE === "1" ? holdOrchestratorLease(orchestratorSocketPath(env)) : undefined;
	const emitBlocked = (data: unknown) => pi.events.emit("herdr:blocked", data);
	const waitSignal = new HerdrWaitSignal(emitBlocked);
	const approvals = new RelayApprovals(new HerdrWaitSignal(emitBlocked));
	let notify: (message: string, type: "info" | "warning") => void = () => {};
	const signIns = new SignInWatcher(signInSurroundings, () => lease, (message, type) => notify(message, type), new HerdrWaitSignal(emitBlocked));
	const promptTypeByWorkId = new Map<string, string>();
	const z = pi.zod;

	function rememberPromptTypes(works: IncompleteWork[]): void {
		for (const work of works) {
			if (work.prompt_type) promptTypeByWorkId.set(work.work_id, work.prompt_type);
		}
	}

	function terminalResult(terminal: OrchestratorEvent, workId: string | undefined, transcript: BoundedTranscript): ToolResult {
		if (terminal.event === "information_requested") {
			waitSignal.begin(INFORMATION_WAIT_LABEL);
			return textResult(renderInformationRequest(terminal, workId), {
				status: "information_requested",
				work_id: workId,
				request_id: terminal.request_id,
				questions: stringList(terminal.questions),
				context: terminal.context,
			});
		}
		if (terminal.event === "completed") {
			if (workId && promptTypeByWorkId.get(workId) === "SOFTWARE_ENGINEERING_PROJECT_PLANNING") {
				waitSignal.begin(PLAN_REVIEW_WAIT_LABEL);
			}
			return textResult(renderCompletion(terminal, workId, transcript), {
				status: "completed",
				work_id: workId,
				summary: terminal.summary,
				artifacts: stringList(terminal.artifacts),
				plan: terminal.plan,
			});
		}
		const code = typeof terminal.code === "string" ? terminal.code : "failed";
		const message = typeof terminal.message === "string" ? terminal.message : "no details were provided";
		const lines = [`Work ${workId ?? "(unknown)"} failed (${code}): ${message}`];
		if (!transcript.isEmpty) lines.push("", "Progress log:", transcript.text());
		return textResult(lines.join("\n"), { status: "failed", work_id: workId, code, message }, true);
	}

	async function runStreamingWork(
		method: string,
		params: Record<string, unknown>,
		onUpdate: UpdateCallback,
		promptType?: string,
	): Promise<ToolResult> {
		const transcript = new BoundedTranscript(TRANSCRIPT_LINE_LIMIT);
		let workId = typeof params.work_id === "string" ? params.work_id : undefined;
		const handleEvent = (event: OrchestratorEvent) => {
			if (typeof event.work_id === "string") {
				workId = event.work_id;
				if (promptType) promptTypeByWorkId.set(workId, promptType);
			}
			if (event.event !== "progress" && event.event !== "agent_output") return;
			transcript.append(formatEventLine(event));
			onUpdate?.({ content: [{ type: "text", text: transcript.text() }], details: { status: "running", work_id: workId } });
		};
		let outcome: StreamOutcome;
		try {
			outcome = await client.stream(method, params, handleEvent);
		} catch (error) {
			return textResult(describeError(error), { status: "error", work_id: workId }, true);
		}
		const accepted = outcome.result as { work_id?: string } | undefined;
		if (typeof accepted?.work_id === "string") {
			workId = accepted.work_id;
			if (promptType) promptTypeByWorkId.set(workId, promptType);
		}
		return terminalResult(outcome.terminal, workId, transcript);
	}

	async function fetchStatus(): Promise<any> {
		const status = await client.request("project.status", {});
		rememberPromptTypes(incompleteWorkOf(status));
		return status;
	}

	pi.registerTool({
		name: "clyean_status",
		label: "Clyean status",
		description:
			"Report whether the Clyean project is scaffolded, its project type, whether it is locked, and any unfinished units of orchestrated work.",
		parameters: z.object({}),
		approval: "read",
		async execute(_toolCallId: string, _params: unknown, _signal: unknown, _onUpdate: UpdateCallback) {
			try {
				const status = await fetchStatus();
				return textResult(renderProjectStatus(status), status);
			} catch (error) {
				return textResult(describeError(error), { status: "error" }, true);
			}
		},
	});

	pi.registerTool({
		name: "clyean_scaffold",
		label: "Clyean scaffold",
		description:
			"Scaffold the current project (Git repository, .clyean directory, agent sandbox, specifications, and architecture) with the given project type. Streams progress and returns when scaffolding is complete.",
		parameters: z.object({
			project_type: z.enum([...PROJECT_TYPES]).describe("The project type you determined from the user's prompt and the project contents."),
		}),
		async execute(_toolCallId: string, params: { project_type: string }, _signal: unknown, onUpdate: UpdateCallback, ctx: HandlerContext) {
			return runStreamingWork("project.scaffold", { project_type: params.project_type, session_id: sessionIdOf(ctx) }, onUpdate);
		},
	});

	pi.registerTool({
		name: "clyean_delegate",
		label: "Clyean delegate",
		description:
			"Hand a software-engineering research, planning, or implementation prompt to the Software Engineering Director through the host orchestrator. Streams progress and returns either the final result or a request for more information from the user.",
		parameters: z.object({
			prompt_type: z.enum([...PROMPT_TYPES]).describe("The prompt type you assigned."),
			refined_prompt: z.string().describe("The user's request, refined and enriched with the context you gathered."),
			original_prompt: z.string().describe("The user's own words, unmodified."),
			plan: z.string().optional().describe("An existing change plan reference to implement, such as 2026-09-21-add-login/v2."),
		}),
		async execute(
			_toolCallId: string,
			params: { prompt_type: string; refined_prompt: string; original_prompt: string; plan?: string },
			_signal: unknown,
			onUpdate: UpdateCallback,
			ctx: HandlerContext,
		) {
			const requestParams: Record<string, unknown> = {
				prompt_type: params.prompt_type,
				prompt: params.refined_prompt,
				original_prompt: params.original_prompt,
				session_id: sessionIdOf(ctx),
			};
			if (params.plan) requestParams.plan = params.plan;
			return runStreamingWork("work.start", requestParams, onUpdate, params.prompt_type);
		},
	});

	pi.registerTool({
		name: "clyean_provide_information",
		label: "Clyean provide information",
		description:
			"Deliver the user's answers to an information request raised by orchestrated work, then continue streaming that work until it completes or asks again.",
		parameters: z.object({
			work_id: z.string().describe("The work_id from the information request."),
			request_id: z.string().describe("The request_id from the information request."),
			answers: z.array(z.string()).describe("One answer per question, in the order the questions were asked."),
		}),
		async execute(
			_toolCallId: string,
			params: { work_id: string; request_id: string; answers: string[] },
			_signal: unknown,
			onUpdate: UpdateCallback,
		) {
			waitSignal.clear();
			return runStreamingWork(
				"work.provide_information",
				{ work_id: params.work_id, request_id: params.request_id, answers: params.answers },
				onUpdate,
			);
		},
	});

	pi.registerTool({
		name: "clyean_resume",
		label: "Clyean resume",
		description: "Resume an unfinished unit of orchestrated work, streaming its progress until it completes or asks for information.",
		parameters: z.object({ work_id: z.string().describe("The work_id to resume.") }),
		async execute(_toolCallId: string, params: { work_id: string }, _signal: unknown, onUpdate: UpdateCallback) {
			return runStreamingWork("work.resume", { work_id: params.work_id }, onUpdate);
		},
	});

	pi.registerTool({
		name: "clyean_cancel",
		label: "Clyean cancel",
		description: "Cancel a unit of orchestrated work.",
		parameters: z.object({ work_id: z.string().describe("The work_id to cancel.") }),
		async execute(_toolCallId: string, params: { work_id: string }) {
			try {
				const result = await client.request("work.cancel", { work_id: params.work_id });
				waitSignal.clear();
				return textResult(`Work ${params.work_id} was cancelled.`, result);
			} catch (error) {
				return textResult(describeError(error), { status: "error", work_id: params.work_id }, true);
			}
		},
	});


	pi.registerTool({
		name: "clyean_relay",
		label: "Clyean relay",
		description:
			"Relay traffic from a port on the user's computer to a port on this container's loopback, so the user's browser, or with host_addresses such as [\"0.0.0.0\"] other machines, can reach a server you run here. Each relay needs the user's approval unless their allowlist covers it; a denial comes back as an error, and you should not ask again unless the user asks. Actions: open (protocol, host_port, guest_port, optional host_addresses and reason), close (relay_id), and list. The server must listen on this container's loopback or on all interfaces.",
		parameters: z.object({
			action: z.enum(["open", "close", "list"]).describe("What to do."),
			protocol: z.enum(["tcp", "udp", "sctp"]).optional().describe("The protocol to relay; tcp when omitted."),
			host_port: z.number().optional().describe("The port on the user's computer, 1 to 65535."),
			guest_port: z.number().optional().describe("The port in this container; the host port when omitted."),
			host_addresses: z
				.array(z.string())
				.optional()
				.describe("Host addresses to listen on; loopback when omitted. 0.0.0.0 means every IPv4 address and :: every IPv6 address."),
			reason: z.string().optional().describe("Why, in a sentence the user reads in the prompt."),
			relay_id: z.string().optional().describe("The relay to close."),
		}),
		async execute(_toolCallId: string, params: any) {
			try {
				if (params.action === "list") {
					const result: any = await client.request("relay.list", {});
					return textResult(renderRelayList(Array.isArray(result?.relays) ? result.relays : []), result);
				}
				if (params.action === "close") {
					if (!params.relay_id) throw new Error("close needs the relay_id of an open relay");
					const result = await client.request("relay.close", { relay_id: params.relay_id });
					return textResult(`Relay ${params.relay_id} is closed.`, result);
				}
				if (typeof params.host_port !== "number") throw new Error("open needs a host_port");
				const request: Record<string, unknown> = {
					protocol: params.protocol ?? "tcp",
					host_port: params.host_port,
					guest_port: params.guest_port ?? params.host_port,
				};
				if (Array.isArray(params.host_addresses) && params.host_addresses.length > 0) request.host_addresses = params.host_addresses;
				if (typeof params.reason === "string" && params.reason.trim()) request.reason = params.reason;
				const result = await client.request("relay.open", request);
				return textResult(renderRelayOpened(result), result);
			} catch (error) {
				return textResult(describeError(error), { status: "error", code: (error as { code?: string })?.code }, true);
			}
		},
	});

	pi.registerTool({
		name: "clyean_server",
		label: "Clyean server",
		description:
			"Run a server (a development server, a database, or any long-running command) in a container of its own on the project network, or stop or list the project's servers. Every agent container of the project, yours included, reaches a server by its name on the ports it listens on, for example http://web:5173/, so the server must listen on all interfaces (0.0.0.0 or ::), not only on localhost; declare every port it listens on. A relay makes a port reachable from the user's computer and needs the user's approval unless their allowlist covers it; do not ask again after a denial. A server stops when this clyean process ends unless persistent is true. Launching the same name with the same parameters returns the running server. The server's output goes to .clyean/logs/servers/<name>.log. A server you only need yourself can instead run in your own container, with clyean_relay for the user to reach it. Actions: launch (name, command, ports, and optionally cwd, persistent, relays, ready_timeout_seconds, reason), stop (name), and list.",
		parameters: z.object({
			action: z.enum(["launch", "stop", "list"]).describe("What to do."),
			name: z.string().optional().describe("The server's name: lower-case letters, digits, and hyphens, up to 32 characters; also its host name on the project network."),
			command: z.string().optional().describe("The shell command that runs the server in the foreground, run with sh -c."),
			cwd: z.string().optional().describe("The working directory, relative to the project directory; the project directory when omitted."),
			persistent: z.boolean().optional().describe("Keep the server running after this clyean process ends, until stopped."),
			ports: z
				.array(z.object({ protocol: z.enum(["tcp", "udp", "sctp"]), port: z.number() }))
				.optional()
				.describe("Every port the server listens on in its container, one to eight."),
			relays: z
				.array(
					z.object({
						protocol: z.enum(["tcp", "udp"]),
						host_port: z.number(),
						guest_port: z.number(),
						host_addresses: z.array(z.string()).optional(),
					}),
				)
				.optional()
				.describe("Up to four relays from a port on the user's computer (loopback unless host_addresses says otherwise) to one of the declared ports."),
			ready_timeout_seconds: z.number().optional().describe("How long to wait for the ports to listen, 1 to 300; 60 when omitted."),
			reason: z.string().optional().describe("Why, in a sentence the user reads if a relay needs approval."),
		}),
		async execute(_toolCallId: string, params: any) {
			try {
				if (params.action === "list") {
					const listing = await client.request("server.list", {});
					return textResult(renderServerList(listing), listing);
				}
				if (params.action === "stop") {
					if (!params.name) throw new Error("stop needs the name of a server");
					const stopped = await client.request("server.stop", { name: params.name });
					return textResult(`Server ${params.name} is stopped.`, stopped);
				}
				const started: any = await serverClient.request("server.launch", serverLaunchParams(params));
				return textResult(String(started?.summary ?? JSON.stringify(started)), started);
			} catch (error) {
				return textResult(describeError(error), { status: "error", code: (error as { code?: string })?.code }, true);
			}
		},
	});

	async function serversCommand(words: string[], ctx: any): Promise<void> {
		const [action, name] = words;
		if (action === "stop") {
			if (!name || words.length !== 2) throw new Error("usage: /clyean servers stop <name>");
			await client.request("server.stop", { name });
			ctx.ui?.notify?.(`Server ${name} is stopped.`, "info");
			return;
		}
		if (action) throw new Error("usage: /clyean servers [stop <name>]");
		const listing: any = await client.request("server.list", {});
		ctx.ui?.notify?.(renderServerList(listing), "info");
		const servers: any[] = Array.isArray(listing?.servers) ? listing.servers : [];
		if (servers.length === 0 || ctx?.hasUI !== true || typeof ctx.ui?.select !== "function") return;
		const stopChoices = servers.map(server => `Stop ${server.name}`);
		const choice = await ctx.ui.select("Servers", [...stopChoices, "Done"]);
		const server = servers[stopChoices.indexOf(choice ?? "")];
		if (!server) return;
		await client.request("server.stop", { name: server.name });
		ctx.ui?.notify?.(`Server ${server.name} is stopped.`, "info");
	}

	/** Tells the user which of the project's servers already run, persistent ones above all. */
	async function announceRunningServers(ctx: HandlerContext): Promise<void> {
		let listing: any;
		try {
			listing = await client.request("server.list", {});
		} catch {
			return;
		}
		const servers: any[] = Array.isArray(listing?.servers) ? listing.servers : [];
		if (servers.length === 0) return;
		const described = servers.map(server => `${server.name}${server.persistent ? " (persistent)" : ""}`);
		ctx.ui?.notify?.(
			`Servers running for this project: ${described.join(", ")}. Run /clyean servers to see where they listen or to stop one.`,
			"info",
		);
	}

	async function relaysCommand(words: string[], ctx: any): Promise<void> {
		const request = lease?.request;
		if (!request) {
			throw new Error("relays are managed by the clyean process that started this container, and its lease is not held");
		}
		const sessionId = sessionIdOf(ctx);
		const show = (listing: unknown) => ctx.ui?.notify?.(renderAllowlist(listing), "info");
		const [action, ...rest] = words;
		if (action === "allow") {
			const { scope, entry } = parseAllowCommand(rest);
			show(await request("relay.allowlist.add", { scope, entry, session_id: sessionId }));
			return;
		}
		if (action === "revoke") {
			const [scope, number] = rest;
			const index = Number(number) - 1;
			if (!["project", "session"].includes(scope ?? "") || !Number.isInteger(index) || index < 0 || rest.length !== 2) {
				throw new Error("usage: /clyean relays revoke <project|session> <number>");
			}
			show(await request("relay.allowlist.remove", { scope, index, session_id: sessionId }));
			return;
		}
		if (action) throw new Error("usage: /clyean relays [allow <project|session> <protocols> <host ports> <guest ports> <addresses> | revoke <project|session> <number>]");
		const listing: any = await request("relay.allowlist.list", { session_id: sessionId });
		show(listing);
		if (ctx?.hasUI !== true || typeof ctx.ui?.select !== "function") return;
		const choice = await ctx.ui.select("Relays", ["Add an entry", "Remove an entry", "Close a relay", "Done"]);
		if (choice === "Add an entry") {
			const scope = await ctx.ui.select("Allow for", ["This project", "This session"]);
			if (!scope) return;
			const protocols = await ctx.ui.input("Protocols: tcp, udp, or sctp, comma-separated", "tcp");
			if (protocols === undefined) return;
			const hostPorts = await ctx.ui.input("Host ports: 3000, 3000-3010, or any", "any");
			if (hostPorts === undefined) return;
			const guestPorts = await ctx.ui.input("Guest ports: 3000, 3000-3010, or any", "any");
			if (guestPorts === undefined) return;
			const preset = await ctx.ui.select("Host addresses", [...Object.keys(ADDRESS_PRESETS), "Other addresses"]);
			if (!preset) return;
			let addresses = ADDRESS_PRESETS[preset];
			if (!addresses) {
				const typed = await ctx.ui.input("Host addresses, comma-separated", "127.0.0.1");
				if (!typed) return;
				addresses = typed.split(",").map((address: string) => address.trim());
			}
			const entry = {
				protocols: (protocols.trim() || "tcp").split(",").map((protocol: string) => protocol.trim().toLowerCase()),
				hostPorts: hostPorts.trim() || "any",
				guestPorts: guestPorts.trim() || "any",
				hostAddresses: addresses,
			};
			show(await request("relay.allowlist.add", { scope: scope === "This session" ? "session" : "project", entry, session_id: sessionId }));
		} else if (choice === "Remove an entry") {
			const entries = [
				...(listing?.project ?? []).map((entry: any, index: number) => ({ label: `Project: ${entry.text}`, scope: "project", index })),
				...(listing?.session ?? []).map((entry: any, index: number) => ({ label: `Session: ${entry.text}`, scope: "session", index })),
			];
			if (entries.length === 0) return;
			const picked = await ctx.ui.select("Remove which entry?", entries.map(entry => entry.label));
			const entry = entries.find(candidate => candidate.label === picked);
			if (entry) show(await request("relay.allowlist.remove", { scope: entry.scope, index: entry.index, session_id: sessionId }));
		} else if (choice === "Close a relay") {
			const relays: any[] = Array.isArray(listing?.relays) ? listing.relays : [];
			if (relays.length === 0) return;
			const labels = renderRelayList(relays).split("\n");
			const picked = await ctx.ui.select("Close which relay?", labels);
			const relay = relays[labels.indexOf(picked ?? "")];
			if (relay) {
				await request("relay.close", { relay_id: relay.relay_id });
				ctx.ui?.notify?.(`Relay ${relay.relay_id} is closed.`, "info");
			}
		}
	}

	pi.registerCommand("clyean", {
		description:
			"Show Clyean project status and unfinished orchestrated work; `/clyean sign-in <agent> <server>` signs in to an MCP server for another agent; `/clyean relays` shows and edits relays and their allowlists; `/clyean servers` lists and stops server containers",
		handler: async (args: string, ctx: any) => {
			const words = (args ?? "").trim().split(/\s+/).filter(Boolean);
			const [subcommand, agent, server] = words;
			try {
				if (subcommand === "relays") {
					await relaysCommand(words.slice(1), ctx);
					return;
				}
				if (subcommand === "servers") {
					await serversCommand(words.slice(1), ctx);
					return;
				}
				if (subcommand === "sign-in") {
					if (!agent || !server) throw new Error("usage: /clyean sign-in <agent> <server>");
					const url = await signInOnBehalf(
						{ agent, server, projectDir: env.CLYEAN_PROJECT_DIR ?? ctx.cwd ?? process.cwd() },
						ctx.modelRegistry.authStorage,
						await authorityHarness(),
						{
							onAuth: info =>
								ctx.ui?.notify?.(
									`Open this address to sign in to ${server} for the ${agent} agent: ${info.url}\nIf the browser cannot return to Clyean, paste the address it ends on.`,
									"info",
								),
							onManualCodeInput: async () =>
								(await ctx.ui?.input?.("Paste the address your browser ended on after signing in")) ?? "",
						},
					);
					ctx.ui?.notify?.(`Signed in to ${server} (${url}); the ${agent} agent receives a copy when it next starts.`, "info");
					return;
				}
				const status = await fetchStatus();
				ctx.ui?.notify?.(renderProjectStatus(status), "info");
			} catch (error) {
				ctx.ui?.notify?.(describeError(error), "error");
			}
		},
	});

	pi.on("input", (event: any) => {
		waitSignal.clear();
		const text = typeof event?.text === "string" ? event.text.trim() : "";
		if (lease && startsSignIn(text)) signIns.open();
		return undefined;
	});

	function bindSession(ctx: HandlerContext): void {
		notify = (message, type) => ctx.ui?.notify?.(message, type);
		if (!lease) return;
		bindLeaseShutdown(lease, ctx);
		bindLeaseAuthority(lease, ctx, authorityHarness, approvals);
	}

	pi.on("session_switch", (_event, ctx: HandlerContext) => {
		bindSession(ctx);
	});

	pi.on("session_start", async (_event, ctx: HandlerContext) => {
		bindSession(ctx);
		// A sign-in can follow startup without any typed command, as the first-run setup does.
		if (lease && ctx?.hasUI) signIns.open();
		void announceRunningServers(ctx);
		let status: any;
		try {
			status = await fetchStatus();
		} catch {
			// The orchestrator may legitimately be absent (for example in print mode without a host).
			return;
		}
		const incomplete = incompleteWorkOf(status);
		if (incomplete.length === 0) return;
		const ids = incomplete.map(work => work.work_id);
		ctx.ui?.notify?.(
			`Clyean found ${incomplete.length} unfinished unit${incomplete.length === 1 ? "" : "s"} of work (${ids.join(", ")}). Run /clyean for details or ask to resume.`,
			"warning",
		);
		pi.sendMessage(
			{
				customType: "clyean-incomplete-work",
				content: [
					"Clyean's orchestrator reports unfinished work from an earlier turn of this session:",
					...incomplete.map(work => `- ${work.work_id}: ${work.prompt_type ?? "unknown type"}${work.phase ? `, phase ${work.phase}` : ""}`),
					"",
					"Confirm with the user whether to continue. Call clyean_resume with the work_id to finish it, or clyean_cancel to abandon it.",
				].join("\n"),
				display: false,
			},
			{ deliverAs: "nextTurn" },
		);
	});
}
