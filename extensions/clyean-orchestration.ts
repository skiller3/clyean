// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
// CLYEAN_EXTENSION_VERSION=2
// managed by clyean; upgrading clyean overwrites this file.
// add user customizations in sibling files instead of editing this one.
//
// Connects the Clyean User Assistant to the host orchestrator, which the bridge of
// the clyean process that started this container serves at the orchestrator
// socket.  The protocol is documented in docs/reference/orchestrator-protocol.md.

import net from "node:net";

type Env = Record<string, string | undefined>;
type Timer = ReturnType<typeof setTimeout>;
type TextContent = { type: "text"; text: string };
type ToolResult = { content: TextContent[]; details?: unknown; isError?: boolean };
type UpdateCallback = ((partialResult: ToolResult) => void) | undefined;
type HandlerContext = {
	hasUI?: boolean;
	sessionManager?: { getSessionId?: () => string | undefined };
	ui?: { notify?: (message: string, type?: "info" | "warning" | "error") => void };
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

const USER_ASSISTANT_AGENT_ID = "user-assistant";
const TERMINAL_EVENTS = new Set(["information_requested", "completed", "failed"]);
const TRANSCRIPT_LINE_LIMIT = 400;
const DEFAULT_CONNECT_TIMEOUT_MS = 5000;
const DEFAULT_SILENCE_TIMEOUT_MS = 10 * 60 * 1000;

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
}

/**
 * Holds one connection to the orchestrator for the life of this process.  The clyean
 * process that started this container serves it through its bridge, so the connection
 * ends exactly when that process is gone, for whatever reason, and the harness must then
 * shut down.  The socket does not keep the process alive on its own.
 */
export function holdOrchestratorLease(socketPath: string): OrchestratorLease {
	const holder = globalThis as Record<symbol, Map<string, OrchestratorLease> | undefined>;
	const leases = (holder[LEASES] ??= new Map());
	const existing = leases.get(socketPath);
	if (existing) return existing;
	const lease: OrchestratorLease = {};
	leases.set(socketPath, lease);
	const lose = (reason: string) => {
		if (lease.lost) return;
		lease.lost = reason;
		lease.shutdown?.();
	};
	let buffered = "";
	let answered = false;
	const socket = net.createConnection(socketPath);
	socket.unref();
	socket.setEncoding("utf8");
	socket.on("connect", () => {
		socket.write(`${JSON.stringify({ id: `lease:${process.pid}`, method: LEASE_METHOD, params: {} })}\n`);
	});
	socket.on("data", (chunk: string) => {
		if (answered) return;
		buffered += chunk;
		const newline = buffered.indexOf("\n");
		if (newline === -1) return;
		answered = true;
		let frame: any;
		try {
			frame = JSON.parse(buffered.slice(0, newline));
		} catch {
			frame = undefined;
		}
		if (!frame || frame.error) {
			lose(`the orchestrator refused the lease (${frame?.error?.message ?? "the answer was not JSON"})`);
			socket.destroy();
		}
	});
	socket.on("error", () => {});
	socket.on("close", () => lose("the clyean process that started this container is gone"));
	return lease;
}

function bindLeaseShutdown(lease: OrchestratorLease, ctx: any): void {
	if (typeof ctx?.shutdown !== "function") return;
	lease.shutdown = () => {
		ctx.ui?.notify?.(`Clyean is shutting down: ${lease.lost}.`, "error");
		ctx.shutdown();
	};
	if (lease.lost) lease.shutdown();
}

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

export default function clyeanOrchestration(pi: ExtensionApiLike): void {
	const env: Env = process.env;
	if (env.CLYEAN_AGENT && env.CLYEAN_AGENT !== USER_ASSISTANT_AGENT_ID) return;

	const client = new OrchestratorClient(orchestratorSocketPath(env), {
		connectTimeoutMs: parseDurationEnv(env, "CLYEAN_ORCHESTRATOR_CONNECT_TIMEOUT_MS", DEFAULT_CONNECT_TIMEOUT_MS),
		silenceTimeoutMs: parseDurationEnv(env, "CLYEAN_ORCHESTRATOR_SILENCE_TIMEOUT_MS", DEFAULT_SILENCE_TIMEOUT_MS),
	});
	const lease = env.CLYEAN_ORCHESTRATOR_LEASE === "1" ? holdOrchestratorLease(orchestratorSocketPath(env)) : undefined;
	const waitSignal = new HerdrWaitSignal(data => pi.events.emit("herdr:blocked", data));
	const promptTypeByWorkId = new Map<string, string>();
	const z = pi.zod;

	function rememberPromptTypes(works: IncompleteWork[]): void {
		for (const work of works) {
			if (work.prompt_type) promptTypeByWorkId.set(work.work_id, work.prompt_type);
		}
	}

	function sessionIdOf(ctx: HandlerContext | undefined): string | undefined {
		try {
			const id = ctx?.sessionManager?.getSessionId?.();
			return typeof id === "string" && id.length > 0 ? id : undefined;
		} catch {
			return undefined;
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

	pi.registerCommand("clyean", {
		description: "Show Clyean project status and unfinished orchestrated work",
		handler: async (_args: string, ctx: HandlerContext) => {
			try {
				const status = await fetchStatus();
				ctx.ui?.notify?.(renderProjectStatus(status), "info");
			} catch (error) {
				ctx.ui?.notify?.(describeError(error), "error");
			}
		},
	});

	pi.on("input", () => {
		waitSignal.clear();
		return undefined;
	});

	pi.on("session_switch", (_event, ctx: HandlerContext) => {
		if (lease) bindLeaseShutdown(lease, ctx);
	});

	pi.on("session_start", async (_event, ctx: HandlerContext) => {
		if (lease) bindLeaseShutdown(lease, ctx);
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
