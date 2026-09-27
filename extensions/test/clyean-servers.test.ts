// Copyright (C) 2026 Skye Isard
// SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception

import { expect, test } from "bun:test";
import { createFakePi, importFresh } from "./harness";

const MODULE = "../clyean-servers.ts";

/** A UI whose `input` records each request and answers with `answer(request)`. */
function recordingUi(answer: (request: any) => unknown) {
	const requests: Array<{ title: string; request: any; options: any }> = [];
	return {
		requests,
		ui: {
			async input(title: string, placeholder?: string, options?: any) {
				const request = JSON.parse(placeholder ?? "{}");
				requests.push({ title, request, options });
				const reply = answer(request);
				return reply === undefined ? undefined : JSON.stringify(reply);
			},
		},
	};
}

async function installed(env: Record<string, string | undefined> = { CLYEAN_WORK_ID: "w1" }) {
	const fake = createFakePi();
	const { default: install } = await importFresh(MODULE);
	install(fake.pi, env);
	return fake;
}

test("only orchestrated sub-agents get the tool", async () => {
	expect((await installed({})).tools.size).toBe(0);
	expect([...(await installed()).tools.keys()]).toEqual(["clyean_server"]);
});

test("launches go to the orchestrator as a server request and return where the server listens", async () => {
	const fake = await installed();
	const { requests, ui } = recordingUi(() => ({ type: "server_started", name: "web", ready: true, summary: "Server web is ready." }));
	const result = await fake.tools.get("clyean_server").execute(
		"c1",
		{ action: "launch", name: "web", command: "npm run dev", ports: [{ protocol: "tcp", port: 5173 }], persistent: false },
		undefined,
		undefined,
		{ ui },
	);
	expect(requests[0].title).toBe("clyean:server");
	expect(requests[0].request).toEqual({
		action: "launch",
		name: "web",
		command: "npm run dev",
		ports: [{ protocol: "tcp", port: 5173 }],
		persistent: false,
	});
	expect(requests[0].options.timeout).toBe(30 * 60_000);
	expect(result.content[0].text).toBe("Server web is ready.");
	expect(result.isError).toBeUndefined();
});

test("refusals come back as errors with their codes, and list and stop map to their requests", async () => {
	const fake = await installed();
	const { requests, ui } = recordingUi(request => {
		if (request.action === "launch") {
			return { error: { code: "relay_denied", message: "the user denied the relay; do not ask for it again unless the user asks", reason: "denied" } };
		}
		if (request.action === "list") return { type: "server_list", servers: [{ name: "db", summary: "Server db is ready." }] };
		return { type: "server_stopped", name: request.name };
	});
	const tool = fake.tools.get("clyean_server");
	const denied = await tool.execute(
		"c1",
		{ action: "launch", name: "web", command: "x", ports: [{ protocol: "tcp", port: 1 }], relays: [{ protocol: "tcp", host_port: 3000, guest_port: 1 }] },
		undefined,
		undefined,
		{ ui },
	);
	expect(denied.isError).toBe(true);
	expect(denied.details).toEqual({ status: "error", code: "relay_denied", reason: "denied" });
	expect(denied.content[0].text).toContain("do not ask for it again");
	expect((await tool.execute("c2", { action: "list" }, undefined, undefined, { ui })).content[0].text).toBe("Server db is ready.");
	expect((await tool.execute("c3", { action: "stop", name: "db" }, undefined, undefined, { ui })).content[0].text).toBe("Server db is stopped.");
	expect(requests.map(entry => entry.request.action)).toEqual(["launch", "list", "stop"]);
	const unnamed = await tool.execute("c4", { action: "stop" }, undefined, undefined, { ui });
	expect(unnamed.isError).toBe(true);
	const unanswered = recordingUi(() => undefined);
	const silent = await tool.execute("c5", { action: "list" }, undefined, undefined, { ui: unanswered.ui });
	expect(silent.content[0].text).toBe("the orchestrator did not answer");
	const early = await tool.execute("c6", { action: "list" }, undefined, undefined, {});
	expect(early.content[0].text).toBe("the session has not started yet");
});
