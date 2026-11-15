# Contributions to the public repository

Loca is developed in the private repository `omrylcn/loca-private`. The
public repository `omrylcn/loca` receives an exported tree and the releases
(see the release repository contract in [AGENTS.md](../AGENTS.md)). A pull
request or issue opened there by someone outside is therefore taken into the
private repository, goes through its gates there, and comes back out with
the next export. Nothing is merged in the public repository.

## Who looks

The maintainer reads every new public pull request and issue. An agent may
draft a summary or a patch, but the decision to take a change in or decline
it is the maintainer's. A report of a vulnerability goes to the private
channel in [SECURITY.md](../SECURITY.md): reply only with a pointer there,
and if the report contains a credential, treat it as exposed (SECURITY.md,
"Credential exposure").

## An issue

1. Reproduce it from the public report alone; ask in the issue for what is
   missing, never for credentials or room content.
2. Fix it in the private repository like any other change, with a test that
   fails on the code before the fix. Name the public issue in the private
   pull request.
3. When the export that carries the fix is on the public repository, comment
   with the public commit (and the release, once it is tagged) and close the
   issue.

## A pull request

1. **Read before anything runs.** Read the whole diff on GitHub. Its build
   scripts, tests and workflows are code that runs on whatever machine runs
   them. Approve a first-time contributor's workflow run only after that
   reading.
2. **Decide.** Decline with a short reason and close, or take it in.
3. **Take it in as a patch, keeping the author.** In the private repository,
   on a new branch from current `master`:

   ```bash
   gh pr diff N --repo omrylcn/loca --patch > public-pr-N.patch
   git am --3way public-pr-N.patch
   ```

   `git am` keeps each commit's author and date. The public tree has the same
   paths as the private one, so the patch applies unless the public tree is
   behind; resolve that by hand and keep the author. Your own changes (a
   missing test, a fix to the change) go in separate commits on top; do not
   squash the contributor's commits into yours. Name the public pull request
   (`omrylcn/loca#N`) in the private pull request. If the change adds a file
   that `packaging/public-source-paths.txt` does not already cover, add it
   there in the same pull request, or it is not exported.
4. **Gates.** The same as for every private change: its own branch and pull
   request, a test shown failing on the code before it, the full gate set run
   locally in Docker with the result posted as a comment on the pull request
   with the tested SHA for each gate, and an independent reviewer's GO on the
   exact SHA; when it goes in through an integration branch, that branch's
   rules in [CONTRIBUTING.md](../CONTRIBUTING.md) apply. The reviewer reads the
   contributor's code as untrusted: network access, process spawning,
   workflow and build-script changes, and anything that touches credentials
   or room boundaries.
5. **Out with the next export.** After the merge, the change reaches the
   public repository with the next export from an approved private commit,
   following the release contract. The two repositories share no history, so
   the public commit is the export commit, not the contributor's: put one
   `Co-authored-by: Name <email>` trailer per contributor, with the name and
   email from their patch, in the export commit message, so GitHub credits
   them.
6. **Close with credit.** Comment on the public pull request with the public
   export commit (and the release, once tagged), thank the author, and close
   it. Say that it is closed rather than merged because changes enter
   through the private repository.

## Never

- Merge a public pull request, or push or commit to a public branch, other
  than through an export from an approved private commit.
- Run a contribution's code (build, tests, scripts) on a machine or in a
  workflow that holds secrets, a membership key, an invite, the Building root
  key, a real Building's data, or a token for either repository; add or
  change a `pull_request_target` workflow that checks out contributed code.
- Take a change without its author, rewrite its authorship, or credit only
  yourself.
- Copy into the public discussion anything that is private: private review
  text, Loca data, identities, or files outside the allow-list.
- Ask for, or keep, a credential posted in an issue or a pull request.
