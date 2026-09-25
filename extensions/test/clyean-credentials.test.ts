// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

import { afterEach, beforeEach, expect, test } from "bun:test";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { environmentSandbox, importFresh, waitFor } from "./harness";

const CREDENTIALS_MODULE = "../clyean-credentials.ts";
const MINUTE = 60_000;
const MCP_ID = "mcp_oauth:profile:programmer:https://mcp.example.com/sse";

const sandbox = environmentSandbox();
let directory: string;

beforeEach(() => {
	sandbox.reset();
	directory = fs.mkdtempSync(path.join(os.tmpdir(), "clyean-credentials-"));
});

afterEach(() => {
	fs.rmSync(directory, { recursive: true, force: true });
	sandbox.restore();
});

/** A login store that records what the extension did to it. */
function fakeStore(initial: Record<string, unknown[]> = {}) {
	const rows: Record<string, unknown[]> = { ...initial };
	const log: string[] = [];
	return {
		rows,
		log,
		list: () => Object.keys(rows),
		async remove(provider: string) {
			log.push(`remove ${provider}`);
			delete rows[provider];
		},
		async set(provider: string, credential: unknown) {
			log.push(`set ${provider}`);
			rows[provider] = Array.isArray(credential) ? credential : [credential];
		},
		close() {
			log.push("close");
		},
	};
}

function fakePi() {
	const handlers = new Map<string, Array<(event: any, ctx: any) => unknown>>();
	const providers = new Map<string, any>();
	return {
		providers,
		api: {
			on(event: string, handler: (event: any, ctx: any) => unknown) {
				handlers.set(event, [...(handlers.get(event) ?? []), handler]);
			},
			registerProvider(name: string, config: any) {
				providers.set(name, config);
			},
		},
		async emit(event: string, ctx: any) {
			for (const handler of handlers.get(event) ?? []) await handler({}, ctx);
		},
	};
}

/** The orchestrator's side of renewal requests: records them and answers with `answer`. */
function renewalUi(answer: (wanted: any) => unknown) {
	const requests: Array<{ title: string; wanted: any }> = [];
	return {
		requests,
		async input(title: string, placeholder?: string) {
			const wanted = JSON.parse(placeholder ?? "{}");
			requests.push({ title, wanted });
			return JSON.stringify(answer(wanted));
		},
	};
}

function writeBundle(bundle: unknown): string {
	const file = path.join(directory, "clyean-credentials.json");
	fs.writeFileSync(file, JSON.stringify(bundle));
	sandbox.reset({ CLYEAN_CREDENTIALS_BUNDLE: file });
	return file;
}

async function install(store: ReturnType<typeof fakeStore>) {
	const { createCredentialsExtension } = await importFresh(CREDENTIALS_MODULE);
	const pi = fakePi();
	const apiKeys: string[] = [];
	const extension = createCredentialsExtension(
		{
			openStore: async () => store,
			apiKeyOf: async (provider: string, copy: any) => {
				apiKeys.push(`${provider}:${copy.access}`);
				return provider === "google-gemini-cli" ? JSON.stringify({ token: copy.access }) : undefined;
			},
		},
		process.env,
	);
	await extension(pi.api);
	return { pi, apiKeys };
}

test("the bundle replaces everything in the store and is deleted", async () => {
	const file = writeBundle({
		version: 1,
		providers: { anthropic: [{ type: "oauth", access: "a", refresh: "", expires: Date.now() + 30 * MINUTE }] },
		mcp: { [MCP_ID]: { type: "oauth", access: "m", refresh: "", expires: Date.now() + 30 * MINUTE } },
	});
	const store = fakeStore({ openai: [{ type: "api_key", key: "stale" }], anthropic: [{ type: "oauth", access: "old" }] });
	await install(store);
	expect(store.log).toEqual(["remove openai", "remove anthropic", "set anthropic", `set ${MCP_ID}`, "close"]);
	expect(Object.keys(store.rows).sort()).toEqual(["anthropic", MCP_ID]);
	expect(fs.existsSync(file)).toBe(false);
});

test("without a bundle the store is left alone", async () => {
	const store = fakeStore({ openai: [{ type: "api_key", key: "kept" }] });
	sandbox.reset({ CLYEAN_CREDENTIALS_BUNDLE: path.join(directory, "absent.json") });
	const { pi } = await install(store);
	expect(store.log).toEqual([]);
	expect(pi.providers.size).toBe(0);
	sandbox.reset();
	await install(store);
	expect(store.log).toEqual([]);
});

test("the harness's refresh of an OAuth copy asks the orchestrator instead of the provider", async () => {
	writeBundle({
		version: 1,
		providers: {
			anthropic: [{ type: "oauth", access: "a1", refresh: "", expires: Date.now() + MINUTE, email: "me@x" }],
			openai: [{ type: "api_key", key: "sk" }],
		},
		mcp: {},
	});
	const { pi } = await install(fakeStore());
	expect([...pi.providers.keys()]).toEqual(["anthropic"]);
	const ui = renewalUi(() => ({
		providers: {
			anthropic: [
				{ type: "oauth", access: "other", refresh: "", expires: Date.now() + 60 * MINUTE, email: "someone@x" },
				{ type: "oauth", access: "a2", refresh: "", expires: Date.now() + 60 * MINUTE, email: "me@x" },
			],
		},
		mcp: {},
	}));
	await pi.emit("session_start", { ui });
	const oauth = pi.providers.get("anthropic").oauth;
	const renewed = await oauth.refreshToken({ access: "a1", refresh: "", expires: Date.now(), email: "me@x" });
	expect(renewed.access).toBe("a2");
	expect(renewed.refresh).toBe("");
	expect(ui.requests).toEqual([{ title: "clyean:credentials", wanted: { providers: ["anthropic"] } }]);
	expect(oauth.getApiKey(renewed)).toBe("a2");
	await expect(oauth.login()).rejects.toThrow("from the Clyean User Assistant");
});

test("a refused renewal fails the request with a message naming the provider", async () => {
	writeBundle({
		version: 1,
		providers: { anthropic: [{ type: "oauth", access: "a1", refresh: "", expires: Date.now() + MINUTE }] },
		mcp: {},
	});
	const { pi } = await install(fakeStore());
	await pi.emit("session_start", { ui: renewalUi(() => ({ error: "the User Assistant is signed out of anthropic" })) });
	const oauth = pi.providers.get("anthropic").oauth;
	await expect(oauth.refreshToken({ access: "a1", refresh: "", expires: Date.now() })).rejects.toThrow(
		"Clyean could not renew this agent's anthropic credential: the User Assistant is signed out of anthropic",
	);
});

test("structured API keys are computed once per copy by the harness", async () => {
	writeBundle({
		version: 1,
		providers: { "google-gemini-cli": [{ type: "oauth", access: "g1", refresh: "", expires: Date.now() + 30 * MINUTE }] },
		mcp: {},
	});
	const { pi, apiKeys } = await install(fakeStore());
	expect(apiKeys).toEqual(["google-gemini-cli:g1"]);
	const oauth = pi.providers.get("google-gemini-cli").oauth;
	expect(oauth.getApiKey({ access: "g1" })).toBe(JSON.stringify({ token: "g1" }));
	expect(oauth.getApiKey({ access: "unknown" })).toBe("unknown");
});

test("an MCP copy is renewed before it expires and written to the session's store", async () => {
	writeBundle({
		version: 1,
		providers: {},
		mcp: { [MCP_ID]: { type: "oauth", access: "m1", refresh: "", expires: Date.now() + 10 * MINUTE + 30 } },
	});
	const { pi } = await install(fakeStore());
	const sessionStore = fakeStore();
	const ui = renewalUi(() => ({
		providers: {},
		mcp: { [MCP_ID]: { type: "oauth", access: "m2", refresh: "", expires: Date.now() + 60 * MINUTE } },
	}));
	await pi.emit("session_start", { ui, modelRegistry: { authStorage: sessionStore } });
	await waitFor(() => sessionStore.log.includes(`set ${MCP_ID}`), 2000, "the MCP renewal");
	expect(ui.requests).toEqual([{ title: "clyean:credentials", wanted: { mcp_servers: ["https://mcp.example.com/sse"] } }]);
	expect((sessionStore.rows[MCP_ID][0] as any).access).toBe("m2");
});

test("a bundle that cannot be imported stops the harness", async () => {
	writeBundle({ version: 99 });
	const exit = process.exit;
	const codes: Array<number | undefined> = [];
	(process as any).exit = (code?: number) => {
		codes.push(code);
		throw new Error("exited");
	};
	try {
		await expect(install(fakeStore())).rejects.toThrow("exited");
	} finally {
		process.exit = exit;
	}
	expect(codes).toEqual([78]);
});
