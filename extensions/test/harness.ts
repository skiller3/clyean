// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

import fs from "node:fs";
import { rm } from "node:fs/promises";
import { createServer, type Server, type Socket } from "node:net";
import os from "node:os";
import path from "node:path";

export type Handler = (event: any, ctx: any) => unknown;

export interface FakePi {
	pi: any;
	handlers: Map<string, Handler[]>;
	busHandlers: Map<string, Array<(data: any) => void>>;
	busEmissions: Array<{ channel: string; data: unknown }>;
	tools: Map<string, any>;
	commands: Map<string, any>;
	sentMessages: Array<{ message: any; options: any }>;
	emit(event: string, payload: any, ctx?: any): Promise<unknown>;
}

function chainable(kind: string, extra: Record<string, unknown> = {}): any {
	const schema: any = { kind, ...extra, optionalFlag: false, description: undefined };
	schema.describe = (description: string) => {
		schema.description = description;
		return schema;
	};
	schema.optional = () => {
		schema.optionalFlag = true;
		return schema;
	};
	return schema;
}

export const fakeZod = {
	object: (shape: Record<string, unknown>) => chainable("object", { shape }),
	string: () => chainable("string"),
	number: () => chainable("number"),
	boolean: () => chainable("boolean"),
	enum: (values: readonly string[]) => chainable("enum", { values }),
	array: (item: unknown) => chainable("array", { item }),
};

export function createFakePi(): FakePi {
	const handlers = new Map<string, Handler[]>();
	const busHandlers = new Map<string, Array<(data: any) => void>>();
	const busEmissions: Array<{ channel: string; data: unknown }> = [];
	const tools = new Map<string, any>();
	const commands = new Map<string, any>();
	const sentMessages: Array<{ message: any; options: any }> = [];
	const pi = {
		zod: fakeZod,
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
		events: {
			on(channel: string, handler: (data: any) => void) {
				const list = busHandlers.get(channel) ?? [];
				list.push(handler);
				busHandlers.set(channel, list);
				return () => {};
			},
			emit(channel: string, data: unknown) {
				busEmissions.push({ channel, data });
				for (const handler of busHandlers.get(channel) ?? []) handler(data);
			},
		},
		registerTool(definition: any) {
			tools.set(definition.name, definition);
		},
		registerCommand(name: string, definition: any) {
			commands.set(name, definition);
		},
		sendMessage(message: unknown, options: unknown) {
			sentMessages.push({ message, options });
		},
	};
	return {
		pi,
		handlers,
		busHandlers,
		busEmissions,
		tools,
		commands,
		sentMessages,
		async emit(event: string, payload: any, ctx?: any) {
			let last: unknown;
			for (const handler of handlers.get(event) ?? []) last = await handler(payload, ctx);
			return last;
		},
	};
}

export interface FakeContextOptions {
	hasUI?: boolean;
	idle?: boolean;
	sessionFile?: string;
	sessionId?: string;
}

export function createFakeContext(options: FakeContextOptions = {}) {
	const notifications: Array<{ message: string; type?: string }> = [];
	const ctx = {
		hasUI: options.hasUI ?? true,
		mode: "tui",
		isIdle: () => options.idle ?? true,
		sessionManager: {
			getSessionFile: () => options.sessionFile,
			getSessionId: () => options.sessionId,
		},
		ui: {
			notify(message: string, type?: string) {
				notifications.push({ message, type });
			},
		},
	};
	return { ctx, notifications };
}

/** Unix socket paths are limited to about 108 bytes, so sockets live directly under a short temp root. */
export function shortSocketPath(name: string): string {
	const root = fs.existsSync("/tmp") ? "/tmp" : os.tmpdir();
	const socketPath = path.join(root, `cly-${name}-${process.pid}-${Math.random().toString(36).slice(2, 7)}.sock`);
	if (socketPath.length > 100) throw new Error(`socket path too long for AF_UNIX: ${socketPath}`);
	return socketPath;
}

export function sleep(ms: number): Promise<void> {
	return new Promise(resolve => setTimeout(resolve, ms));
}

export async function waitFor(predicate: () => boolean, timeoutMs = 2000, label = "condition"): Promise<void> {
	const deadline = Date.now() + timeoutMs;
	while (!predicate()) {
		if (Date.now() > deadline) throw new Error(`timed out waiting for ${label}`);
		await sleep(5);
	}
}

export interface RecordingServer {
	socketPath: string;
	requests: any[];
	connections: number;
	frames(method: string): any[];
	close(): Promise<void>;
}

function listen(server: Server, socketPath: string): Promise<void> {
	return new Promise((resolve, reject) => {
		server.once("error", reject);
		server.listen(socketPath, () => resolve());
	});
}

async function closeServer(server: Server, socketPath: string): Promise<void> {
	await new Promise<void>(resolve => server.close(() => resolve()));
	await rm(socketPath, { force: true });
}

/** Stub Herdr socket: records one request per connection and answers with an empty result. */
export async function startRecordingServer(name: string, options: { dropFirstResponse?: boolean } = {}): Promise<RecordingServer> {
	const socketPath = shortSocketPath(name);
	const requests: any[] = [];
	const state = { connections: 0 };
	const server = createServer((socket: Socket) => {
		state.connections += 1;
		const connectionIndex = state.connections;
		let input = "";
		socket.setEncoding("utf8");
		socket.on("data", (chunk: string) => {
			input += chunk;
			const newline = input.indexOf("\n");
			if (newline === -1) return;
			const request = JSON.parse(input.slice(0, newline));
			if (options.dropFirstResponse && connectionIndex === 1) {
				socket.end();
				return;
			}
			requests.push(request);
			socket.end(`${JSON.stringify({ id: request.id, result: {} })}\n`);
		});
		socket.on("error", () => {});
	});
	await listen(server, socketPath);
	return {
		socketPath,
		requests,
		get connections() {
			return state.connections;
		},
		frames(method: string) {
			return requests.filter(request => request.method === method);
		},
		close: () => closeServer(server, socketPath),
	};
}

export interface StubReply {
	result?: unknown;
	error?: { code: string; message: string };
	events?: Array<Record<string, unknown>>;
	eventDelayMs?: number;
	closeAfterEvents?: boolean;
}

export interface StubOrchestrator {
	socketPath: string;
	requests: any[];
	/** Lines the extension wrote after its request, such as answers on the lease. */
	followUps: any[];
	/** Writes a line to every open connection, as the orchestrator does on the lease. */
	send(frame: unknown): void;
	/** Ends every open connection, as the bridge does when its clyean process dies. */
	dropConnections(): void;
	close(): Promise<void>;
}

/** Stub host orchestrator: one request per connection, one response, then optional streamed events. */
export async function startStubOrchestrator(
	name: string,
	handle: (request: any) => StubReply | Promise<StubReply>,
): Promise<StubOrchestrator> {
	const socketPath = shortSocketPath(name);
	const requests: any[] = [];
	const followUps: any[] = [];
	const sockets = new Set<Socket>();
	const server = createServer((socket: Socket) => {
		sockets.add(socket);
		socket.on("close", () => sockets.delete(socket));
		let input = "";
		let handled = false;
		socket.setEncoding("utf8");
		socket.on("error", () => {});
		socket.on("data", async (chunk: string) => {
			input += chunk;
			if (handled) {
				for (let newline = input.indexOf("\n"); newline !== -1; newline = input.indexOf("\n")) {
					followUps.push(JSON.parse(input.slice(0, newline)));
					input = input.slice(newline + 1);
				}
				return;
			}
			const newline = input.indexOf("\n");
			if (newline === -1) return;
			handled = true;
			const request = JSON.parse(input.slice(0, newline));
			input = input.slice(newline + 1);
			requests.push(request);
			const reply = await handle(request);
			if (reply.error) {
				socket.end(`${JSON.stringify({ id: request.id, error: reply.error })}\n`);
				return;
			}
			socket.write(`${JSON.stringify({ id: request.id, result: reply.result ?? {} })}\n`);
			for (const event of reply.events ?? []) {
				if (reply.eventDelayMs) await sleep(reply.eventDelayMs);
				if (socket.destroyed) return;
				socket.write(`${JSON.stringify(event)}\n`);
			}
			if (reply.closeAfterEvents !== false) socket.end();
		});
	});
	await listen(server, socketPath);
	return {
		socketPath,
		requests,
		followUps,
		send(frame: unknown) {
			for (const socket of sockets) socket.write(`${JSON.stringify(frame)}\n`);
		},
		dropConnections() {
			for (const socket of sockets) socket.destroy();
		},
		close() {
			for (const socket of sockets) socket.destroy();
			return closeServer(server, socketPath);
		},
	};
}

const ENVIRONMENT_KEYS = [
	"HERDR_ENV",
	"HERDR_PANE_ID",
	"HERDR_TAB_ID",
	"HERDR_WORKSPACE_ID",
	"HERDR_SOCKET_PATH",
	"HERDR_BIN_PATH",
	"HERDR_CLYEAN_INTEGRATION",
	"OMPCODE",
	"OMP_PROFILE",
	"PI_CODING_AGENT_DIR",
	"CLYEAN_AGENT",
	"CLYEAN_PROJECT_DIR",
	"CLYEAN_WORKSPACE_DIR",
	"CLYEAN_HOST_WORKSPACE_DIR",
	"CLYEAN_HOST_CONTAINER_ROOT",
	"CLYEAN_HERDR_IDLE_DEBOUNCE_MS",
	"CLYEAN_HERDR_RETRY_GRACE_MS",
	"CLYEAN_ORCHESTRATOR_SOCKET",
	"CLYEAN_ORCHESTRATOR_CONNECT_TIMEOUT_MS",
	"CLYEAN_ORCHESTRATOR_SILENCE_TIMEOUT_MS",
	"CLYEAN_ORCHESTRATOR_LEASE",
	"CLYEAN_CREDENTIALS_BUNDLE",
] as const;

/** Snapshot the environment keys the extensions read so each test can start clean and restore afterwards. */
export function environmentSandbox() {
	const saved = new Map<string, string | undefined>();
	for (const key of ENVIRONMENT_KEYS) saved.set(key, process.env[key]);
	return {
		reset(values: Record<string, string | undefined> = {}) {
			for (const key of ENVIRONMENT_KEYS) delete process.env[key];
			for (const [key, value] of Object.entries(values)) {
				if (value !== undefined) process.env[key] = value;
			}
		},
		restore() {
			for (const [key, value] of saved) {
				if (value === undefined) delete process.env[key];
				else process.env[key] = value;
			}
		},
	};
}

let importCounter = 0;

/** Import a module fresh so module-level state never leaks between tests. */
export function importFresh<T = any>(modulePath: string): Promise<T> {
	importCounter += 1;
	return import(`${modulePath}?fresh=${importCounter}`) as Promise<T>;
}
