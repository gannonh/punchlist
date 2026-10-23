# 11. Search issues and comments with trigram indexes and ILIKE

Date: 2026-10-23. Status: accepted.

## Context

PRD R19 asks for search across issues and comments. The list's filter matched only what a loaded row carries: the id, title, labels, assignee and status name. A word only in a body or a comment found nothing.

People search an issue tracker for identifiers as often as for words: `PL-12`, `ci_green`, `request_transition`, `pull_request.rs`. Postgres offers two engines.

**Full-text search (`tsvector`).** It tokenizes first. On Postgres 17, `ts_debug('simple', 'PL-12 ci_green')` gives:

```text
   alias   |   description   | token | dictionaries | dictionary | lexemes
-----------+-----------------+-------+--------------+------------+---------
 asciiword | Word, all ASCII | PL    | {simple}     | simple     | {pl}
 int       | Signed integer  | -12   | {simple}     | simple     | {-12}
 blank     | Space symbols   |       | {}           |            |
 asciiword | Word, all ASCII | ci    | {simple}     | simple     | {ci}
 blank     | Space symbols   | _     | {}           |            |
 asciiword | Word, all ASCII | green | {simple}     | simple     | {green}
```

`PL-12` becomes the word `pl` and the signed integer `-12`, and `ci_green` becomes `ci` and `green`. The `english` configuration also stems: `to_tsvector('english', 'running PL-12 ci_green')` is `'-12':3 'ci':4 'green':5 'pl':2 'run':1`. So what matches is not what was typed. Matching `PL-1` to `PL-12` as a person types needs `:*` prefix queries and a hand-built `tsquery`.

**Trigrams (`pg_trgm`).** A GIN index with `gin_trgm_ops` answers `ILIKE '%text%'` for any text of three or more characters. A query matches exactly the substring the person typed, identifiers included, and the snippet can show that same substring.

## Decision

- `GET /api/search?q=` matches one literal phrase, case-insensitively, as a substring of an issue's id, title or body, or of any of its comments, with `ILIKE ... ESCAPE '\'`. `\`, `%` and `_` in the query are escaped, so they match themselves.
- A migration creates the `pg_trgm` extension, a trusted extension, and four GIN trigram indexes: `issue (id)`, `issue (title)`, `issue (body)` and `comment (body)`.
- A query needs 3 to 200 characters after trimming. Under 3, a trigram index cannot narrow the search.
- The server cuts the snippet in Rust from the matched text, so the snippet shows exactly what matched.

## Consequences

- No stemming: `run` does not find `running`, and does find `trunk`.
- No relevance ranking. Hits in the id or title come first, then the rest, each newest issue first.
- The query reads and sorts every issue the text matches before it applies the limit. A common word costs more than a rare one.
- A trigram index is several times the size of its text, and it slows each insert of an issue or a comment.
- The migration user needs `CREATE` on the database. That is enough for a trusted extension.
- When ranking matters, add a generated `tsvector` column per table with a GIN index beside the trigram indexes. Keep `ILIKE` for identifiers.
