// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
// CLYEAN_EXTENSION_VERSION=1
// managed by clyean; upgrading clyean overwrites this file.
// add user customizations in sibling files instead of editing this one.
//
// Gives a Clyean sub-agent the clyean_server tool.  A server it asks for runs in a
// container of its own on the project network, where this agent's container reaches it by
// name, and the user approves any relay that makes it reachable from their computer.  The
// orchestrator launches the server on the agent's behalf when asked through an extension UI
// request with a reserved title.

export const SERVER_REQUEST_TITLE = "clyean:server";
/** Long enough for a launch whose four relays each wait on the user, then on the server. */
const SERVER_REQUEST_TIMEOUT_MS = 30 * 60_000;

type ServerUi = {
	input(title: string, placeholder?: string, options?: { timeout?: number; signal?: AbortSignal }): Promise<string | undefined>;
};
export type ExtensionApiLike = {
	zod: any;
	on(event: string, handler: (event: any, ctx: any) => unknown): void;
	registerTool(definition: any): void;
};
type ToolResult = { content: Array<{ type: "text"; text: string }>; details?: unknown; isError?: boolean };

export class ServerRequestError extends Error {
	constructor(
		message: string,
		readonly code?: string,
		readonly reason?: string,
	) {
		super(message);
	}
}

/** Asks the orchestrator to launch, stop, or list servers, and returns its answer. */
export async function requestServers(
	ui: ServerUi | undefined,
	request: Record<string, unknown>,
	signal?: AbortSignal,
): Promise<any> {
	if (!ui) throw new ServerRequestError("the session has not started yet");
	const answer = await ui.input(SERVER_REQUEST_TITLE, JSON.stringify(request), { timeout: SERVER_REQUEST_TIMEOUT_MS, signal });
	if (answer === undefined) throw new ServerRequestError("the orchestrator did not answer");
	const parsed = JSON.parse(answer);
	if (parsed?.error) {
		throw new ServerRequestError(String(parsed.error.message ?? "the request failed"), parsed.error.code, parsed.error.reason ?? undefined);
	}
	return parsed;
}

/** The `server.launch` parameters among the tool's. */
export function launchParams(params: any): Record<string, unknown> {
	if (!params?.name || !params?.command) throw new ServerRequestError("launch needs a name and a command");
	if (!Array.isArray(params.ports) || params.ports.length === 0) {
		throw new ServerRequestError("launch needs the ports the server listens on, such as [{\"protocol\": \"tcp\", \"port\": 5173}]");
	}
	const request: Record<string, unknown> = { name: params.name, command: params.command, ports: params.ports };
	for (const key of ["cwd", "persistent", "relays", "ready_timeout_seconds", "reason"]) {
		if (params[key] !== undefined) request[key] = params[key];
	}
	return request;
}

export function renderServerList(listing: any): string {
	const servers: any[] = Array.isArray(listing?.servers) ? listing.servers : [];
	if (servers.length === 0) return "No servers are running for this project.";
	return servers.map(server => String(server.summary ?? server.name)).join("\n");
}

function textResult(text: string, details?: unknown, isError = false): ToolResult {
	const result: ToolResult = { content: [{ type: "text", text }], details };
	if (isError) result.isError = true;
	return result;
}

export default function clyeanServers(pi: ExtensionApiLike, env: Record<string, string | undefined> = process.env): void {
	if (!env.CLYEAN_WORK_ID) return;
	const z = pi.zod;
	let sessionUi: ServerUi | undefined;
	pi.on("session_start", (_event, ctx: any) => {
		sessionUi = ctx?.ui;
	});

	pi.registerTool({
		name: "clyean_server",
		label: "Clyean server",
		description:
			"Run a server (a development server, a database, or any long-running command) in a container of its own on the project network, or stop or list servers. Your container reaches a server by its name on the ports it listens on, for example http://web:5173/, so the server must listen on all interfaces (0.0.0.0 or ::), not only on localhost; declare every port it listens on. A relay makes a port reachable from the user's computer and needs the user's approval: ask for one only when the user or your task needs to reach the server from outside the sandbox, and do not ask again after a denial. A server stops when the user's clyean process ends unless persistent is true. Launching the same name with the same parameters returns the running server. The server's output goes to .clyean/logs/servers/<name>.log. Actions: launch (name, command, ports, and optionally cwd, persistent, relays, ready_timeout_seconds, reason), stop (name), and list.",
		parameters: z.object({
			action: z.enum(["launch", "stop", "list"]).describe("What to do."),
			name: z.string().optional().describe("The server's name: lower-case letters, digits, and hyphens, up to 32 characters; also its host name on the project network."),
			command: z.string().optional().describe("The shell command that runs the server in the foreground, run with sh -c."),
			cwd: z.string().optional().describe("The working directory, relative to the project directory; the project directory when omitted."),
			persistent: z.boolean().optional().describe("Keep the server running after the user's clyean process ends, until stopped."),
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
		async execute(_toolCallId: string, params: any, signal: AbortSignal | undefined, _onUpdate: unknown, ctx: any) {
			const ui: ServerUi | undefined = ctx?.ui ?? sessionUi;
			try {
				if (params.action === "list") {
					const listing = await requestServers(ui, { action: "list" }, signal);
					return textResult(renderServerList(listing), listing);
				}
				if (params.action === "stop") {
					if (!params.name) throw new ServerRequestError("stop needs the name of a server");
					const stopped = await requestServers(ui, { action: "stop", name: params.name }, signal);
					return textResult(`Server ${params.name} is stopped.`, stopped);
				}
				const started = await requestServers(ui, { action: "launch", ...launchParams(params) }, signal);
				return textResult(String(started?.summary ?? JSON.stringify(started)), started);
			} catch (error) {
				const failure = error instanceof ServerRequestError ? error : new ServerRequestError(String((error as Error)?.message ?? error));
				return textResult(failure.message, { status: "error", code: failure.code, reason: failure.reason }, true);
			}
		},
	});
}
