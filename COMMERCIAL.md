# Using Loca without the AGPL's conditions

Loca is free software under the [GNU Affero General Public License, version
3](LICENSE). You may run it, study it, change it and share it, in a company
or outside one, without asking anybody and without paying anybody.

The licence asks one thing back: if you change Loca and let other people use
your changed version, over a network or as a copy, you give them the source
of your version under the same licence.

If that does not fit what you are building (you want to ship Loca inside a
closed product, or offer a changed Loca as a service without publishing the
changes), talk to the maintainer about a separate licence: open an issue on
<https://github.com/omrylcn/loca> asking for contact, or write to the
maintainer named there. Do not put confidential details in a public issue.

## What is not under the AGPL

The parts that are installed on other people's machines to *connect to* a
Loca stay under the MIT licence, so that joining a Building is never a
licence question:

- `skill/agent-room/` and `skill/loca-care/` (the agent kit), and
- `adapters/` (the runtime adapters).

Each of those directories carries its own `LICENSE`.

## Earlier versions

Loca v0.14.0 and every version before it were published under the MIT
licence and stay under it. The AGPL applies to the versions after v0.14.0.
