// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
// CLYEAN_EXTENSION_VERSION=1
// managed by clyean; upgrading clyean overwrites this file.
// add user customizations in sibling files instead of editing this one.
//
// Gives a Clyean sub-agent the credential copies the orchestrator delivered, and keeps
// them fresh.  While the harness loads extensions, before it decides which models are
// available, the copies in the bundle file named by CLYEAN_CREDENTIALS_BUNDLE replace
// everything in this agent's login store, and the file is deleted.  Copies carry no
// refresh tokens: whenever the harness would refresh a model provider's copy, and ten
// minutes before an MCP copy expires, this extension asks the orchestrator for a renewed
// copy instead, through an extension UI request with a reserved title.

import fs from "node:fs";

type Env = Record<string, string | undefined>;

export type Credential = Record<string, unknown> & { type: string };
export type OAuthCopy = Credential & {
	type: "oauth";
	access: string;
	refresh: string;
	expires: number;
	accountId?: string;
	email?: string;
};

export interface CredentialBundle {
	providers: Record<string, Credential[]>;
	mcp: Record<string, Credential>;
}

/** The part of the harness's login store this extension uses. */
export interface LoginStore {
	list(): string[];
	remove(provider: string): Promise<void>;
	set(provider: string, credential: Credential | Credential[]): Promise<void>;
	close?(): void;
}

/** The harness facilities this extension needs, injectable for tests. */
export interface CredentialsHarness {
	/** A store over this agent's login store, apart from the one the session will use. */
	openStore(pi: ExtensionApiLike): Promise<LoginStore>;
	/** The API key the harness sends for an OAuth credential, when it is not the token. */
	apiKeyOf(provider: string, copy: OAuthCopy): Promise<string | undefined>;
}

type RenewalUi = {
	input(title: string, placeholder?: string, options?: { timeout?: number }): Promise<string | undefined>;
};
export type ExtensionApiLike = {
	pi?: any;
	on(event: string, handler: (event: any, ctx: any) => unknown): void;
	registerProvider(name: string, config: any): void;
};

export const RENEWAL_REQUEST_TITLE = "clyean:credentials";
const RENEWAL_TIMEOUT_MS = 60_000;
const MCP_RENEWAL_LEAD_MS = 10 * 60_000;
const MCP_RETRY_MS = 60_000;
const MCP_CREDENTIAL_PREFIX = "mcp_oauth:profile:";

/** Reads a bundle; a missing file means there is nothing to import. */
export function readBundle(path: string): CredentialBundle | undefined {
	let text: string;
	try {
		text = fs.readFileSync(path, "utf8");
	} catch (error: any) {
		if (error?.code === "ENOENT") return undefined;
		throw error;
	}
	const bundle = JSON.parse(text);
	if (bundle?.version !== 1) throw new Error(`the credential bundle has unsupported version ${bundle?.version}`);
	return { providers: bundle.providers ?? {}, mcp: bundle.mcp ?? {} };
}

/** Replaces everything in the store with the bundle's copies: a sub-agent never signs in itself, so everything its store holds is a copy. */
export async function importBundle(store: LoginStore, bundle: CredentialBundle): Promise<void> {
	for (const provider of store.list()) await store.remove(provider);
	for (const [provider, copies] of Object.entries(bundle.providers)) {
		if (copies.length > 0) await store.set(provider, copies);
	}
	for (const [credentialId, copy] of Object.entries(bundle.mcp)) await store.set(credentialId, copy);
}

/** The server address inside a profile-scoped MCP credential identifier. */
export function mcpServerUrl(credentialId: string): string | undefined {
	if (!credentialId.startsWith(MCP_CREDENTIAL_PREFIX)) return undefined;
	const separator = credentialId.indexOf(":", MCP_CREDENTIAL_PREFIX.length);
	return separator === -1 ? undefined : credentialId.slice(separator + 1);
}

/** Asks the orchestrator, which asks the User Assistant, for renewed copies. */
export async function requestRenewal(
	ui: RenewalUi | undefined,
	wanted: { providers?: string[]; mcp_servers?: string[] },
): Promise<CredentialBundle> {
	if (!ui) throw new Error("the session has not started yet");
	const answer = await ui.input(RENEWAL_REQUEST_TITLE, JSON.stringify(wanted), { timeout: RENEWAL_TIMEOUT_MS });
	if (answer === undefined) throw new Error("the orchestrator did not answer");
	const parsed = JSON.parse(answer);
	if (typeof parsed?.error === "string") throw new Error(parsed.error);
	return { providers: parsed?.providers ?? {}, mcp: parsed?.mcp ?? {} };
}

/** The renewed copy of `current` among `copies`: the same account when the credential names one. */
export function matchingCopy(
	copies: Credential[] | undefined,
	current: { accountId?: string; email?: string },
): OAuthCopy | undefined {
	const oauth = (copies ?? []).filter((copy): copy is OAuthCopy => copy.type === "oauth");
	if (current.accountId || current.email) {
		return oauth.find(
			copy =>
				(current.accountId !== undefined && copy.accountId === current.accountId) ||
				(current.email !== undefined && copy.email === current.email),
		);
	}
	return oauth[0];
}

function messageOf(error: unknown): string {
	return error instanceof Error ? error.message : String(error);
}

/** The state renewals share: the UI channel to the orchestrator and precomputed API keys. */
class Renewals {
	ui: RenewalUi | undefined;
	readonly #apiKeys = new Map<string, string>();

	constructor(private readonly harness: CredentialsHarness) {}

	async remember(provider: string, copy: OAuthCopy): Promise<void> {
		try {
			const apiKey = await this.harness.apiKeyOf(provider, copy);
			if (apiKey !== undefined) this.#apiKeys.set(copy.access, apiKey);
		} catch {
			// The access token itself serves as the API key for most providers.
		}
	}

	apiKey(credentials: { access: string }): string {
		return this.#apiKeys.get(credentials.access) ?? credentials.access;
	}

	/** Routes the harness's refresh of `provider`'s copies to the orchestrator. */
	register(pi: ExtensionApiLike, provider: string): void {
		pi.registerProvider(provider, {
			oauth: {
				name: provider,
				login: async () => {
					throw new Error(`this agent receives ${provider} sign-ins from the Clyean User Assistant; sign in there with /login`);
				},
				refreshToken: async (credentials: OAuthCopy) => {
					let renewed: OAuthCopy | undefined;
					try {
						renewed = matchingCopy((await requestRenewal(this.ui, { providers: [provider] })).providers[provider], credentials);
					} catch (error) {
						throw new Error(`Clyean could not renew this agent's ${provider} credential: ${messageOf(error)}`);
					}
					if (!renewed) {
						throw new Error(`Clyean could not renew this agent's ${provider} credential: the User Assistant no longer holds it; sign in there again`);
					}
					await this.remember(provider, renewed);
					return renewed;
				},
				getApiKey: (credentials: OAuthCopy) => this.apiKey(credentials),
			},
		});
	}

	/** Renews an MCP copy ten minutes before it expires, retrying each minute until it does. */
	scheduleMcp(store: LoginStore, credentialId: string, copy: Credential): void {
		const expires = Number(copy.expires);
		const serverUrl = mcpServerUrl(credentialId);
		if (!serverUrl || !Number.isFinite(expires) || expires <= 0) return;
		const renewAt = (at: number) => {
			const timer = setTimeout(async () => {
				try {
					const renewed = (await requestRenewal(this.ui, { mcp_servers: [serverUrl] })).mcp[credentialId];
					if (!renewed) throw new Error("the User Assistant no longer holds it");
					await store.set(credentialId, renewed);
					this.scheduleMcp(store, credentialId, renewed);
				} catch {
					if (Date.now() + MCP_RETRY_MS < expires) renewAt(Date.now() + MCP_RETRY_MS);
				}
			}, Math.max(at - Date.now(), 0));
			timer.unref?.();
		};
		renewAt(expires - MCP_RENEWAL_LEAD_MS);
	}
}

/** The extension, over the given harness facilities. */
export function createCredentialsExtension(harness: CredentialsHarness, env: Env = process.env) {
	return async function clyeanCredentials(pi: ExtensionApiLike): Promise<void> {
		const bundlePath = env.CLYEAN_CREDENTIALS_BUNDLE;
		if (!bundlePath) return;
		let bundle: CredentialBundle | undefined;
		try {
			bundle = readBundle(bundlePath);
			if (bundle) {
				const store = await harness.openStore(pi);
				try {
					await importBundle(store, bundle);
				} finally {
					store.close?.();
				}
				fs.rmSync(bundlePath, { force: true });
			}
		} catch (error) {
			// Running without the delivered credentials would fail later and less clearly.
			console.error(`Clyean could not import this agent's credentials: ${messageOf(error)}`);
			process.exit(78);
		}
		if (!bundle) return;
		const renewals = new Renewals(harness);
		for (const [provider, copies] of Object.entries(bundle.providers)) {
			const oauthCopies = copies.filter((copy): copy is OAuthCopy => copy.type === "oauth");
			if (oauthCopies.length === 0) continue;
			for (const copy of oauthCopies) await renewals.remember(provider, copy);
			renewals.register(pi, provider);
		}
		const imported = bundle;
		pi.on("session_start", (_event, ctx: any) => {
			renewals.ui = ctx?.ui;
			const store: LoginStore | undefined = ctx?.modelRegistry?.authStorage;
			if (!store) return;
			for (const [credentialId, copy] of Object.entries(imported.mcp)) renewals.scheduleMcp(store, credentialId, copy);
		});
	};
}

const harness: CredentialsHarness = {
	openStore: pi => pi.pi.discoverAuthStorage(),
	async apiKeyOf(provider, copy) {
		const { getOAuthApiKey } = await import("@oh-my-pi/pi-ai/oauth");
		return (await getOAuthApiKey(provider, { [provider]: copy }))?.apiKey;
	},
};

export default createCredentialsExtension(harness);
