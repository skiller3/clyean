# Setting up CLA enforcement

This repository enforces the [Contributor License Agreement](../CLA.md) with
[CLA Assistant Lite](https://github.com/contributor-assistant/github-action),
which runs entirely inside GitHub Actions — there is no external service and no
database. The workflow is `.github/workflows/cla.yml`.

Three of the required steps cannot live in a file in the repository and must be
done once, by hand, by the repository owner.

## How it works

1. Someone opens a pull request. The `CLA Assistant` workflow runs on the
   `pull_request_target` event.
2. The action collects every commit author on the pull request and checks each
   one against `signatures/version1/cla.json` on the `cla-signatures` branch.
3. Anyone unsigned gets a pull request comment asking them to sign, and the
   `CLA Assistant` status check **fails**.
4. The contributor posts a comment containing exactly
   `I have read the CLA Document and I hereby sign the CLA`. The action records
   their GitHub username, ID, pull request number, and a timestamp in the
   signature file, and the status check turns green.
5. A branch ruleset makes that status check **required**, so a pull request
   cannot be merged into `main` until every author has signed.
6. After merge, the action locks the pull request conversation so signature
   comments cannot be edited or deleted.

Signing once covers all of that contributor's future pull requests, until the
CLA version changes (see "Revising the CLA" below).

## One-time setup

### 1. Create the `cla-signatures` branch

The action commits signatures to a branch, and **that branch must already
exist and must not be protected**. Keeping signatures off `main` means the
ruleset in step 3 does not block the bot's own commits.

```bash
git switch --orphan cla-signatures
git commit --allow-empty -m "chore(cla): initialise CLA signature store"
git push -u origin cla-signatures
git switch main
```

`git switch --orphan` empties the working tree, and switching back restores it.
In a checkout that carries a large vendored tree (`vendor/omp` is roughly
183 MB), that is two full rewrites of the working tree in order to create a
commit containing nothing. The same branch can be built with plumbing instead,
which touches no files and needs no clean working tree:

```bash
empty_tree=$(git mktree </dev/null)
commit=$(git commit-tree "$empty_tree" -m "chore(cla): initialise CLA signature store")
git push origin "$commit":refs/heads/cla-signatures
```

Both recipes produce the same branch: a single commit with no parent and an
empty tree, authored by whoever runs it.

Do **not** create `signatures/version1/cla.json` yourself. A hand-created file
makes the action fail.

The action creates that file itself, on its **first run against any pull
request**, and not on the first signature. It is created even when no signature
is required, such as when every committer on the pull request is covered by
`allowlist`. That bootstrapping run then **fails**, reporting that committers
have to sign the CLA, because the action creates the store and evaluates
against it in the same pass.

Creating the branch in step 1 is therefore necessary but not sufficient. Expect
the first `CLA Assistant` check after setup to go red once, then re-run it. The
re-run finds the file in place and passes.

### 2. Allow Actions to write to the repository

Settings → Actions → General → Workflow permissions →
**Read and write permissions**. The workflow also declares the permissions it
needs, but that declaration cannot exceed the repository-level setting.

While you are there, under Settings → Actions → General → Fork pull request
workflows, leave the default behaviour that requires approval for first-time
contributors. `pull_request_target` runs with the base repository's token, so
the workflow must never check out or execute code from the fork — this one
does not.

### 3. Require the status check

Import the ruleset in `.github/rulesets/require-cla.json`:

- **UI:** Settings → Rules → Rulesets → New ruleset → Import a ruleset, then
  select that file.
- **CLI:**
  ```bash
  gh api -X POST /repos/skiller3/clyean/rulesets \
    --input .github/rulesets/require-cla.json
  ```

The ruleset requires a status check named `CLA Assistant` — the `name:` of the
job in `.github/workflows/cla.yml`. If you rename that job, rename the context
here too.

The ruleset has no bypass actors, so it applies to you as well: changes to
`main` go through a pull request. If you would rather push directly, add
yourself under **Bypass list** in the ruleset UI, or add to the JSON before
importing:

```json
"bypass_actors": [
  { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" }
]
```

(`actor_id: 5` is the repository admin role.)

## Maintenance

**Allowlist.** `allowlist: skiller3,bot*` in the workflow skips the CLA check
for the repository owner — who already holds copyright — and for bot accounts,
which cannot sign. Add other accounts only if the Owner already holds their
contributions' copyright by some other means, such as a countersigned
Corporate CLA.

**Re-running a check.** Anyone can comment `recheck` on a pull request to make
the action re-evaluate signatures.

**Reviewing signatures.** The signature file is the legal record:

```bash
git show cla-signatures:signatures/version1/cla.json
```

Back it up along with the repository. Never rewrite the history of the
`cla-signatures` branch.

**Corporate CLAs.** A contributor whose employer owns their work needs a
countersigned Corporate CLA (CLA section 6(c)) before they contribute. Handle
that out of band, then add the account to the allowlist, or have them sign in
the pull request as usual once the corporate paperwork is done.

## Revising the CLA

If you publish a materially different CLA, bump the version in `CLA.md` and
change `path-to-signatures` to a new directory, for example
`signatures/version2/cla.json`. Existing contributors will then be asked to
sign the new version, while the version 1 signatures stay on record for the
contributions they covered. Contributions already submitted remain governed by
the CLA version in force when they were submitted (CLA section 9).

## Pinning

The workflow pins `contributor-assistant/github-action` to the commit SHA of
`v2.6.1` rather than to a moving tag, so a compromised or retagged release
cannot change what runs against this repository's write-scoped token. When you
bump it, update both the SHA and the version in the trailing comment.
