# ADR-0006: Plugin licence exception — draft wording

- **Status:** Accepted (as a draft; the wording **must be reviewed by a lawyer before the first public release**)
- **Date:** 2026-11-04
- **Spec:** `docs/SPEC.md §15.6` (Licensing), §2; closes §12 item 7 (drafting)
- **Issue:** Nerdtisan/Locus#50

> **Not legal advice.** This ADR records an engineering draft so the intent is fixed early and reviewers have concrete text to work from. Nothing here takes effect until a reviewed version is added to the repository's licence files.

## Context

The Locus core is AGPL-3.0-or-later (§2). Plugins are WebAssembly components that run inside the host (`wasmtime`) and talk to it only through the published WIT interface `locus:plugin` (§15.1, §15.3). Whether such a component is a "work based on" the AGPL program is legally unclear (§15.6). The decision in principle is already taken (§0, §12.7): add an explicit exception so plugins that use only the published interface may carry **any** licence, permissive or proprietary, while the core stays AGPL. `locus-plugin-api` (WIT + guest SDK) is itself MIT OR Apache-2.0, so depending on the SDK carries no copyleft.

GPLv3/AGPLv3 §7 allows copyright holders to grant **additional permissions**. Two well-known precedents:

- **GCC Runtime Library Exception 3.1** — permission to combine the runtime with independent code "compiled through an Eligible Compilation Process", under any licence. Precise definitions, but tied to compilation.
- **GNU Classpath exception** — permission to link "independent modules" that communicate with the library, regardless of their licence; the library itself stays under the GPL. Short and close to our case, but phrased for linking.

## Options

### Option A — Classpath-style "independent module" exception, adapted to a WIT interface

Defines the interface and what counts as a plugin, then grants permission to combine and convey. Short, familiar to reviewers, matches the architecture (an explicit, versioned interface boundary).

### Option B — GCC-style exception with a defined "eligible" process

Defines eligibility by how the plugin is built (e.g. compiled against only the published WIT). More precise in some respects but awkward: plugins may be written in any language and built with any toolchain (§15.1), so a *build-process* test is the wrong axis. The boundary that matters is *what the plugin talks to at run time*.

### Option C — no exception; rely on the interface being a non-derivative boundary

Zero text, maximum uncertainty for plugin authors — the problem §15.6 exists to remove. Rejected.

## Measurements

No spike: this is a drafting task. Evidence used: the text of the AGPLv3 (§7 additional permissions, §13 remote network interaction), the GCC Runtime Library Exception 3.1 and the GNU Classpath exception (structure only, not copied), and the Locus architecture in §15.

| Criterion | A (Classpath-style) | B (GCC-style) | C (none) |
|---|---|---|---|
| Matches the real boundary (run-time WIT interface) | yes | partly (build-time) | — |
| Works for any plugin language | yes | awkward | — |
| Clarity for plugin authors | good | good | poor |
| Length / review cost | short | longer | none |

## Decision

**Option A.** The draft text follows. It is an AGPLv3 §7 additional permission, granted by the copyright holders of Locus. The CLA (§2) must give the project the right to grant it over contributed code; check that the CLA wording covers it before any external contribution lands.

### Draft text — "Locus Plugin Exception", version 1.0 (DRAFT)

> **Locus Plugin Exception, version 1.0 — DRAFT, NOT YET IN EFFECT, SUBJECT TO LEGAL REVIEW**
>
> This exception is an additional permission under section 7 of the GNU Affero General Public License, version 3 ("AGPLv3"). It applies to Locus, which the copyright holders license under the AGPLv3 or (at your option) any later version.
>
> **0. Definitions.**
>
> "Locus" means the program distributed by the Locus project under the AGPLv3 with this exception, including any version of it that a recipient conveys with this exception still attached.
>
> "Plugin Interface" means the WebAssembly Interface Type (WIT) definitions of the `locus:plugin` package, in any version published by the Locus project as part of `locus-plugin-api`, together with the guest software development kit published with them. A WIT definition that is not published by the Locus project, including a modified copy of the Plugin Interface, is not the Plugin Interface.
>
> "Plugin" means a WebAssembly component, together with its manifest and accompanying data, that (a) interacts with Locus only by importing and exporting functions and types defined in the Plugin Interface, and (b) does not contain, and is not based on, any part of Locus other than the Plugin Interface.
>
> "Host" means Locus, or a modified version of Locus that keeps this exception, when it loads and runs Plugins.
>
> **1. Grant of additional permission.**
>
> You may create, use, modify and convey a Plugin under terms of your choice, and you may load and run a Plugin in a Host, and convey a Host together with Plugins, without the Plugin, or the combination of the Host and the Plugin, becoming subject to the AGPLv3 because of that loading, running or conveying. Running a Plugin inside a Host that users interact with over a network (AGPLv3 section 13) does not make the Plugin subject to the AGPLv3 either.
>
> **2. What stays under the AGPLv3.**
>
> This exception does not change the licence of Locus itself. Locus, and any modified version of it, remain covered by the AGPLv3, including its source-code and network-interaction requirements. A work that uses an interface of Locus other than the Plugin Interface, or that contains or is based on code of Locus other than the Plugin Interface, is not a Plugin and receives no permission under this exception.
>
> **3. Modified versions.**
>
> If you modify Locus, you may extend this exception to your modified version, but you are not obliged to. If you do not wish to, delete this exception statement from your version. If your modified version changes or extends the Plugin Interface, a component that relies on your changes is not a Plugin under this exception unless the Locus project publishes those changes as part of the Plugin Interface.
>
> **4. No other permission.**
>
> This exception grants no trademark rights and no permission beyond what is stated here. It does not affect the licence of `locus-plugin-api`, which is separately available under the MIT licence or the Apache License 2.0.

### Placement (when it takes effect)

- Text in a top-level `LICENSE-EXCEPTION` file; `README.md` and the licence notice in `LICENSE`'s header point to it.
- SPDX expression for core crates: `AGPL-3.0-or-later WITH LicenseRef-Locus-Plugin-Exception` (no SPDX-listed exception fits). Until then core files keep `AGPL-3.0-or-later`; don't change the `// SPDX-License-Identifier:` lines before the reviewed text is added.
- `deny.toml` needs no change: the exception widens what *others* may do with Locus, not which dependencies Locus may use.

## Consequences

- §15.6 now points to this draft; §12.7 records that wording is drafted and legal review is outstanding.
- **Follow-up (blocking the first public release):** legal review of the wording above, then add `LICENSE-EXCEPTION`, update SPDX identifiers and confirm the CLA covers granting it. Tracked in Nerdtisan/Locus#83 (milestone M4).
- Questions for the reviewer, recorded so they aren't lost:
  1. Does "interacts with Locus only by importing and exporting functions and types defined in the Plugin Interface" cover a plugin that also embeds third-party AGPL/GPL code of its own? (Intended: yes — that code's own licence still applies to the plugin; the exception only removes obligations arising from Locus.)
  2. Is the AGPLv3 §13 sentence in clause 1 effective and needed, given plugins are run by the host rather than offered over the network themselves?
  3. AGPLv3 §7 lets downstream recipients remove additional permissions from their copies. Is clause 3's wording ("you may extend… not obliged") the right way to handle forks?
  4. Does including the guest SDK in "Plugin Interface" sit well with the SDK also being MIT OR Apache-2.0?
  5. Does the "or any later version" grant of Locus need the exception to say which AGPL versions it attaches to?
- **What would change this:** advice from the reviewer that a different structure is clearer or safer (for example a GCC-style "eligible" test), or a change to the plugin architecture that adds a second interface (e.g. a browser plugin runtime, §0) — the definition of Plugin Interface would then need to cover it.
