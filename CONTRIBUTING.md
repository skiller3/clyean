# Contributing to Clyean

Thanks for your interest in Clyean. This document covers the legal side of
contributing; project conventions live alongside the code as it lands.

## The short version

- **You must sign the [Contributor License Agreement](CLA.md) before your pull
  request can be merged.** It is signed by posting one comment on your pull
  request, once, ever.
- The CLA **assigns copyright in your contribution to Skye Isard**, the sole
  copyright owner of Clyean.
- You keep a broad, perpetual, irrevocable licence back to your own
  contribution (CLA section 4). You can reuse, republish, and relicense your
  own work elsewhere.
- **Anything you generate by *using* Clyean is yours**, not the project's, and
  carries no AGPL obligation. See
  [LICENSE-EXCEPTION-OUTPUT.md](LICENSE-EXCEPTION-OUTPUT.md). The CLA has
  nothing to do with output; it governs contributions to Clyean's own source.

## Signing the CLA

1. Open a pull request as usual.
2. The **CLA Assistant** workflow comments on it if you have not signed yet,
   and the `CLA Assistant` status check fails.
3. Read [CLA.md](CLA.md), then post a new comment on the pull request
   containing exactly:

   ```
   I have read the CLA Document and I hereby sign the CLA
   ```

4. The check turns green and your signature is recorded on the
   `cla-signatures` branch. You will not be asked again.

If the check is stale, comment `recheck`.

Every commit author on a pull request must have signed — including co-authors
listed in `Co-Authored-By` trailers. If you rebase or amend commits under a
different email, make sure the GitHub account is the same one that signed.

## Before you open a pull request

Confirm each of these, because signing the CLA is you representing them (CLA
section 6):

- The work is yours to give. If your employer could claim it, get their
  written waiver or arrange a Corporate CLA first
  (skye.isard@gmail.com).
- Anything in your contribution that you did not write yourself is clearly
  identified in the pull request, with its source and its licence — including
  code reproduced by an AI tool where you have reason to think it reproduces
  someone else's work.
- No part of it is under a licence that conflicts with assigning copyright.

If you want to share something without assigning it — a suggestion, an
illustrative snippet — mark it conspicuously as **"Not a Contribution"** when
you post it, and do not put it in a pull request.

## Licensing of what you write

Contributions become part of Clyean and are distributed under the GNU Affero
General Public License v3 with the Clyean Generated Output Exception. Because
the Owner holds copyright in the whole project, the Owner may also license
Clyean under other terms, including commercial terms.

New source files should carry a header naming the owner and the licence:

```
Copyright (C) 2026 Skye Isard
SPDX-License-Identifier: AGPL-3.0-only WITH LicenseRef-clyean-output-exception
```

Do not add your own copyright line to a file — under the CLA the copyright is
assigned. Contributors are credited through git history and the pull request
record.

## Reporting a security issue

Do not open a public issue. Email skye.isard@gmail.com.
