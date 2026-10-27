# PL-170 live checks on branch feature/pl-170 (run pl170live)

## Check 1 (main half)
```
$ git -C main rev-parse --short HEAD: fe5deb6
$ main: pl auth login --server http://127.0.0.1:42303
error: unrecognized subcommand 'auth'

Usage: pl [OPTIONS] <COMMAND>

For more information, try '--help'.
--- exit 2
```

## Checks 1 (branch), 2, 3, 5, 6, 7, 8, 9, 10
```
CHECK 1 branch prints a URL and a code: PASS — Open this URL in a browser signed in to Punchlist and approve the code:

  http://127.0.0.1:33471/cli-login?code=NNFXFCNL

Code: NNFXFCNL
Waiting for approval…
screenshot check9-sign-in.png
CHECK 9 signed out goes through /sign-in and back: PASS — went to http://127.0.0.1:33471/sign-in, came back to http://127.0.0.1:33471/cli-login?code=NNFXFCNL, heading "Sign in pl on sartre as Verify Person?"
screenshot check2-approval-page.png
$ psql -c "SELECT count(*) FROM session WHERE kind = 'cli'"
0

$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-a/config.toml auth login --server http://127.0.0.1:33471
--- stdout
Open this URL in a browser signed in to Punchlist and approve the code:

  http://127.0.0.1:33471/cli-login?code=NNFXFCNL

Code: NNFXFCNL
Waiting for approval…
Signed in as Verify Person. Wrote /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-a/config.toml.
--- stderr
--- exit 0
screenshot check2-approved.png
/home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-a/config.toml keys: server_url, token
$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-a/config.toml issue list
--- stdout
No issues.
--- stderr
--- exit 0
$ psql -c "SELECT kind, user_agent FROM session ORDER BY created_at"
browser | Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/153.0.8010.12 Safari/537.36
browser | Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/153.0.8010.12 Safari/537.36
browser | Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/153.0.8010.12 Safari/537.36
cli | pl/0.1.0 (sartre)

$ psql -c "SELECT count(*) FROM session WHERE kind = 'cli'"
1

CHECK 2 approve writes the config within 4 s: PASS — pl exited 0 0.8 s after Approve; pl issue list exit 0; cli rows 0 → 1; rows: browser | Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/153.0.8010.12 Safari/537.36; browser | Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/153.0.8010.12 Safari/537.36; browser | Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/153.0.8010.12 Safari/537.36; cli | pl/0.1.0 (sartre)
CHECK 8 once only: PASS — second approve 409 {"code":"conflict","message":"login NNFXFCNL was already approved"}; wrong secret 404 {"code":"not_found","message":"no login has this secret"}
$ psql -c "SELECT public_id FROM session WHERE kind = 'cli' ORDER BY created_at DESC LIMIT 1"
80d49205-5a32-4f27-ae78-1d56b0af6c6e

screenshot check5-sessions-list.png
screenshot check5-session-ended.png
$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-a/config.toml issue list
--- stdout
--- stderr
error: the server refused pl's token or session; run pl auth login: missing or unknown bearer token (401 Unauthorized)
--- exit 1
CHECK 5 sessions list shows and ends pl: PASS — devices ["pl on sartre","Chrome on Linux","Chrome on Linux","Chrome on Linux"]; next pl issue list exit 1: error: the server refused pl's token or session; run pl auth login: missing or unknown bearer token (401 Unauthorized)
$ psql -c "SELECT count(*) FROM session"
3

$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-b/config.toml auth login --server http://127.0.0.1:33471
--- stdout
Open this URL in a browser signed in to Punchlist and approve the code:

  http://127.0.0.1:33471/cli-login?code=MKCFPPZG

Code: MKCFPPZG
Waiting for approval…
--- stderr
error: the login was denied; nothing was written
--- exit 1
screenshot check3-denied.png
$ psql -c "SELECT count(*) FROM session"
3

CHECK 3 deny: PASS — pl exit 1: error: the login was denied; nothing was written; session rows 3 → 3; /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-b/config.toml exists: false
CHECK 7 browser session as bearer: PASS — GET /api/issues with Bearer <browser session id> → 401 {"code":"unauthorized","message":"missing or unknown bearer token"}; the same id as the cookie → 200
screenshot check10-phone-approval.png
$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-c/config.toml auth login --server http://127.0.0.1:33471
--- stdout
Open this URL in a browser signed in to Punchlist and approve the code:

  http://127.0.0.1:33471/cli-login?code=LQPNVWGD

Code: LQPNVWGD
Waiting for approval…
Signed in as Verify Person. Wrote /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-c/config.toml.
--- stderr
--- exit 0
screenshot check10-phone-approved.png
CHECK 10 phone: PASS — buttons inside 390 px: true; scrollWidth 390, clientWidth 390; pl exit 0
$ psql -c "SELECT public_id, kind FROM session WHERE kind = 'cli' ORDER BY created_at"
8e3344e1-7378-4f66-a5d3-97377475673a | cli

$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-c/config.toml auth logout
--- stdout
Signed out pl on sartre. Run pl auth login to sign in again.
--- stderr
--- exit 0
$ psql -c "SELECT public_id, kind FROM session WHERE kind = 'cli' ORDER BY created_at"

$ pl --config /home/gannonh/.local/share/punchlist/runner/tmp/1c0e2656-1cd3-405e-9247-8735de32734c/3b1b8d94-bc16-4180-8d50-f1c611eeea43-1/pl170-c/config.toml issue list
--- stdout
--- stderr
error: the server refused pl's token or session; run pl auth login: missing or unknown bearer token (401 Unauthorized)
--- exit 1
CHECK 6 logout: PASS — logout exit 0: Signed out pl on sartre. Run pl auth login to sign in again.; cli rows before [8e3344e1-7378-4f66-a5d3-97377475673a | cli], after []; next call exit 1: error: the server refused pl's token or session; run pl auth login: missing or unknown bearer token (401 Unauthorized)
1 PASS, 9 PASS, 2 PASS, 8 PASS, 5 PASS, 3 PASS, 7 PASS, 10 PASS, 6 PASS
```

## Check 4 (expiry)
```
$ (server restarted with PUNCHLIST_CLI_LOGIN_LIFETIME=5) pl --config $TMPDIR/pl170-d/config.toml auth login --server http://127.0.0.1:33471
Open this URL in a browser signed in to Punchlist and approve the code:

  http://127.0.0.1:33471/cli-login?code=HWKVHCVF

Code: HWKVHCVF
Waiting for approval…
error: the login expired; run pl auth login again
--- exit 1 after 6094 ms
config written: no
cli sessions: 0
```
