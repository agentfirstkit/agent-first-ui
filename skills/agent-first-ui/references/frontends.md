# Frontend overrides

## Workspace trust and global configuration

`afui frontend` manages interface replacements — Provider pages, and AFUI's own
session list under `afui frontend init afui sessions --frontend-id custom`. That page is held to the
same contract as any other, with one thing worth saying out loud when a person
asks for it: its URL carries the credential for every session on the machine, so
an override is markup and CSS only and the trust gate is the whole guardrail.
Enable only when the person has explicitly authorized that change; editing or
checking a replacement does not authorize enabling it. Trust is pinned
to an exact content fingerprint for workspace overrides: workspace code stays ignored until someone
enables that fingerprint, and any later edit revokes it until it is enabled
again. A session that is already running keeps serving the bytes that were
trusted when it started — editing a file does not change what is on screen, and
it does not sneak past the gate either.
Workspace resolution verifies the retained snapshot's fingerprint, including
its manifest, after capture. A directory hash from before capture is insufficient
to authorize bytes that may have changed in between. `check` and `enable` refuse
a frontend that contains a symlink: a fingerprint covers files, not the places
they point at, so replace the link with the file itself.

Global frontends (`--scope global`) are trusted user configuration and load
without a workspace trust record. Installing or editing one affects the next
session directly; do not use the global scope to bypass workspace approval.
`enable` and `disable` accept only workspace scope. To suppress global as well
as workspace overrides, use `AFUI_SAFE_MODE=1`; to remove a global replacement,
remove its files within the person's requested scope.

That revocation is the guardrail, not an obstacle. After you edit an override,
do not re-enable it merely to make a check pass. Show the changed content and
enable it only within the person's authorization. `AFUI_SAFE_MODE=1` ignores overrides
entirely and is the right setting when you only need the Provider's default.

`dev` and `preview` locate and validate an implementation for the Provider's own
typed server. Neither renders templates nor starts a generic domain server; if
you expected a running app, you wanted the Provider's binary.

An override is a page, never behaviour. AFUI refuses a frontend file whose name
says it is a script and refuses a script hidden inside a template, so the only
code a page runs is the Provider's own, admitted by its page policy (normally a
per-session nonce). A Provider may also serve its own script files under
`UiPageScript::SameOrigin`; an override still cannot supply them. Write
what a control is *for* using the Provider's declared hook and let the Provider
bind what it does; a control the Provider does not recognise binds to nothing,
and a page missing a control the Provider requires is reported as a broken
override rather than opened.

The same guard refuses markup that runs no script but still sends the person,
a request or the page's address somewhere: `<meta http-equiv>`,
`<meta name=referrer>`, `referrerpolicy`, the form attributes `action`,
`formaction`, `method` and `formmethod`, and `<base>`, `<frame>`,
`<frameset>` and `<applet>`. When a check reports one, remove it rather than
looking for a spelling the guard misses: a form in an override is read by the
Provider's runtime and never posts by itself. In a template you supply,
`javascript:`, `vbscript:` and `data:` are refused anywhere in the file, even in
a `title`, because a template can move text into an attribute; only the page a
template renders is read attribute by attribute.

Files are overlaid one at a time. Supply only the ones you are changing: a
directory is not replaced, so a Provider's remaining templates, partials and
language packs keep serving. Replacing a whole tree instead is how a one-page
customisation silently deletes localisation.

`assets/app-icon.svg` is part of that same overlay. It identifies the Provider
frontend being shown, so it follows the page into its browser favicon and Dock
icon. Do not replace it with an AFUI-wide icon: only AFUI's own `afui sessions`
frontend uses the AFUI icon. If you customise the asset, its bytes participate
in the same trusted fingerprint as every other frontend file.

If a Provider says a frontend will not load, it did not fall back — nothing you
are looking at is the override, and nothing is the Provider's page either. Fix
the frontend, or set `AFUI_SAFE_MODE=1` to get the Provider's own interface
back. A frontend that is merely untrusted is silent by design; read the
Provider's readiness event for a frontend id to tell "mine is running" from
"mine is inert".
