# PL-86 live checks

Runs: branch `pl86-branch` (commit 2da61ac), main `pl86-main` (origin/main 1242c5c). Debug builds on sartre, headless Chromium 1280x800.

## ts_debug on postgres:17
```
   alias   |   description   | token | dictionaries | dictionary | lexemes 
-----------+-----------------+-------+--------------+------------+---------
 asciiword | Word, all ASCII | PL    | {simple}     | simple     | {pl}
 int       | Signed integer  | -12   | {simple}     | simple     | {-12}
 blank     | Space symbols   |       | {}           |            | 
 asciiword | Word, all ASCII | ci    | {simple}     | simple     | {ci}
 blank     | Space symbols   | _     | {}           |            | 
 asciiword | Word, all ASCII | green | {simple}     | simple     | {green}
(6 rows)

               to_tsvector               
-----------------------------------------
 '-12':3 'ci':4 'green':5 'pl':2 'run':1
(1 row)

```

## Browser checks 1, 5, 6, 7, 8 (assertions)
```
PASS 1 main {"count":"1 of 5","visible":["PL-2"],"route":404}
PASS 1 branch {"count":"3 of 5","visible":["PL-1","PL-2","PL-4"],"route":200}
PASS 5 short queries {"requests":0,"count":"1 of 5","visible":["PL-2"],"afterThird":["?q=ci_"]}
PASS 6 snippets {"pl1":"bodyThe runner restarts and leaks ci_green on every retry.","pl1Mark":"ci_green","pl2":null,"pl3":null,"pl4":"body<img src=x onerror=\"window.__x=1\"> ci_green","pl4Img":null,"x":"undefined","withinMs":77}
PASS 6 agents and clear {"agents":"0 of 5","cleared":{"snippets":0,"count":"5 of 5"}}
PASS 7 board and phone {"whiteSpace":"nowrap","phoneBoard":{"fits":true,"snippets":2},"phoneList":{"fits":true,"snippets":2}}
PASS 8 Enter on a comment hit {"path":"/issues/PL-3","hash":"#seq-5","text":"VPVerify Personcommented2mWe saw ciXgreen in the log and 100% of retries pass.","inView":true}
PASS 8 Enter on a body hit {"path":"/issues/PL-1","hash":""}
PASS 8 reload keeps the comment in view {"path":"/issues/PL-3","hash":"#seq-5","text":"VPVerify Personcommented2mWe saw ciXgreen in the log and 100% of retries pass.","inView":true}
```

## Checks 2, 3, 4, 5 (API), 9
```
## Check 2
$ q=ci_green | [issue_id, in_title, snippet.field]
[["PL-2",true,null],["PL-4",false,"body"],["PL-1",false,"body"]]
$ q=CI_GREEN
[["PL-2",true,null],["PL-4",false,"body"],["PL-1",false,"body"]]
$ q=CI_GREEN PL-1 snippet
["The runner restarts and leaks ","ci_green"," on every retry."]
$ q=pl-1
[["PL-1",true]]
## Check 3
$ q=100%25
[["PL-3","comment",5]]
$ q=%25%25%25
{"hits":[],"more":false}
## Check 4
Created workspace 6947ba36-6e5f-48e8-a848-6e24d9f951d3 and person e41c4f66-6fff-4f8a-b9f0-b2a1277cca2e. Save this as ~/.config/punchlist/config.toml; the token is not shown again.
OT-1
$ first workspace q=ci_green ids
["PL-2","PL-4","PL-1"]
$ second workspace q=ci_green ids
OT-1  ci_green elsewhere
["OT-1"]
$ agent token
403
## Check 9
PL-2  Reword ci_green
PL-4  Escape test
    body: <img src=x onerror="window.__x=1"> [ci_green]
PL-1  Gate the transition
    body: The runner restarts and leaks [ci_green] on every retry.
exit=0
No matches.
exit=0
exit=1 stderr=error: search needs at least 3 characters (422 Unprocessable Entity)
ESC count: 0
PL-5  Red
    body:  [31m[red] [0m
 seq 
-----
   5
(1 row)

exit=0
## Check 5 (API)
{"code":"invalid","message":"search needs at least 3 characters"} 422
```

## Check 10: perf
```
branch q=needle-7731: n=100 p50=3.9ms p95=5.2ms mean=4.0ms
branch q=the: n=100 p50=78.2ms p95=80.3ms mean=75.4ms
{"hits":[["PL-1006","comment","needle-7731"],["PL-106","comment","needle-7731"]],"more":false}
{"hits":50,"more":true}
pl86-main comments: n=200 p50=3.8ms p95=4.3ms mean=3.8ms
pl86-branch comments: n=200 p50=4.0ms p95=4.9ms mean=4.1ms
{"label":"pl86-main","rows":2005,"median":"40.2","min":"27.5","max":"45.6","searches":0}
{"label":"pl86-branch","rows":2005,"median":"43.4","min":"30.1","max":"46.5","searches":0}
{"label":"pl86-main","rows":2005,"median":"39.6","min":"29.9","max":"52.8","searches":0}
{"label":"pl86-branch","rows":2005,"median":"41.2","min":"28.5","max":"45.0","searches":0}
```

## Check 10: EXPLAIN (ANALYZE)
```
=== q=needle-7731 statement 1 (matching issues)
                                                                                     QUERY PLAN                                                                                     
------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 Limit  (cost=374.73..374.74 rows=3 width=1135) (actual time=0.294..0.296 rows=2 loops=1)
   ->  Sort  (cost=374.73..374.74 rows=3 width=1135) (actual time=0.293..0.294 rows=2 loops=1)
         Sort Key: (((i.id ~~* '%needle-7731%'::text) OR (i.title ~~* '%needle-7731%'::text))) DESC, i.created_at DESC, i.number DESC
         Sort Method: quicksort  Memory: 29kB
         ->  Nested Loop  (cost=350.05..374.71 rows=3 width=1135) (actual time=0.257..0.270 rows=2 loops=1)
               ->  Unique  (cost=349.78..349.79 rows=3 width=32) (actual time=0.228..0.230 rows=2 loops=1)
                     ->  Sort  (cost=349.78..349.78 rows=3 width=32) (actual time=0.228..0.229 rows=3 loops=1)
                           Sort Key: issue.id
                           Sort Method: quicksort  Memory: 25kB
                           ->  Append  (cost=246.99..349.75 rows=3 width=32) (actual time=0.194..0.207 rows=3 loops=1)
                                 ->  Bitmap Heap Scan on issue  (cost=246.99..251.01 rows=1 width=7) (actual time=0.100..0.100 rows=0 loops=1)
                                       Recheck Cond: ((id ~~* '%needle-7731%'::text) OR (title ~~* '%needle-7731%'::text) OR (body ~~* '%needle-7731%'::text))
                                       Filter: (workspace_id = '1323dfa1-2f47-458b-b5f4-b7de6e0b22d4'::uuid)
                                       ->  BitmapOr  (cost=246.99..246.99 rows=1 width=0) (actual time=0.097..0.097 rows=0 loops=1)
                                             ->  Bitmap Index Scan on issue_id_trgm  (cost=0.00..82.27 rows=1 width=0) (actual time=0.025..0.025 rows=0 loops=1)
                                                   Index Cond: (id ~~* '%needle-7731%'::text)
                                             ->  Bitmap Index Scan on issue_title_trgm  (cost=0.00..82.36 rows=1 width=0) (actual time=0.036..0.036 rows=0 loops=1)
                                                   Index Cond: (title ~~* '%needle-7731%'::text)
                                             ->  Bitmap Index Scan on issue_body_trgm  (cost=0.00..82.36 rows=1 width=0) (actual time=0.036..0.036 rows=0 loops=1)
                                                   Index Cond: (body ~~* '%needle-7731%'::text)
                                 ->  Nested Loop  (cost=82.65..98.73 rows=2 width=7) (actual time=0.093..0.106 rows=3 loops=1)
                                       ->  Bitmap Heap Scan on comment c  (cost=82.37..90.13 rows=2 width=7) (actual time=0.073..0.082 rows=3 loops=1)
                                             Recheck Cond: (body ~~* '%needle-7731%'::text)
                                             Heap Blocks: exact=3
                                             ->  Bitmap Index Scan on comment_body_trgm  (cost=0.00..82.37 rows=2 width=0) (actual time=0.062..0.062 rows=3 loops=1)
                                                   Index Cond: (body ~~* '%needle-7731%'::text)
                                       ->  Index Only Scan using issue_workspace_id_id_key on issue i_1  (cost=0.28..4.30 rows=1 width=7) (actual time=0.007..0.007 rows=1 loops=3)
                                             Index Cond: ((workspace_id = '1323dfa1-2f47-458b-b5f4-b7de6e0b22d4'::uuid) AND (id = c.issue_id))
                                             Heap Fetches: 0
               ->  Index Scan using issue_pkey on issue i  (cost=0.28..8.29 rows=1 width=1133) (actual time=0.007..0.007 rows=1 loops=2)
                     Index Cond: (id = issue.id)
 Planning Time: 1.694 ms
 Execution Time: 0.405 ms
(33 rows)

=== q=the statement 1 (matching issues)
                                                                                         QUERY PLAN                                                                                         
--------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
 Limit  (cost=3321.53..3321.65 rows=51 width=1135) (actual time=63.183..63.191 rows=51 loops=1)
   ->  Sort  (cost=3321.53..3371.70 rows=20071 width=1135) (actual time=63.182..63.188 rows=51 loops=1)
         Sort Key: (((i.id ~~* '%the%'::text) OR (i.title ~~* '%the%'::text))) DESC, i.created_at DESC, i.number DESC
         Sort Method: top-N heapsort  Memory: 195kB
         ->  Hash Join  (cost=2247.87..2651.91 rows=20071 width=1135) (actual time=52.776..62.649 rows=2002 loops=1)
               Hash Cond: (issue.id = i.id)
               ->  HashAggregate  (cost=1868.74..2069.45 rows=20071 width=32) (actual time=49.677..49.880 rows=2002 loops=1)
                     Group Key: issue.id
                     Batches: 1  Memory Usage: 913kB
                     ->  Append  (cost=38.91..1818.56 rows=20071 width=32) (actual time=0.152..47.318 rows=22002 loops=1)
                           ->  Bitmap Heap Scan on issue  (cost=38.91..230.14 rows=81 width=7) (actual time=0.151..9.662 rows=2001 loops=1)
                                 Recheck Cond: ((id ~~* '%the%'::text) OR (title ~~* '%the%'::text) OR (body ~~* '%the%'::text))
                                 Filter: (workspace_id = '1323dfa1-2f47-458b-b5f4-b7de6e0b22d4'::uuid)
                                 Heap Blocks: exact=334
                                 ->  BitmapOr  (cost=38.91..38.91 rows=81 width=0) (actual time=0.123..0.124 rows=0 loops=1)
                                       ->  Bitmap Index Scan on issue_id_trgm  (cost=0.00..12.81 rows=1 width=0) (actual time=0.009..0.009 rows=0 loops=1)
                                             Index Cond: (id ~~* '%the%'::text)
                                       ->  Bitmap Index Scan on issue_title_trgm  (cost=0.00..12.82 rows=1 width=0) (actual time=0.009..0.009 rows=1 loops=1)
                                             Index Cond: (title ~~* '%the%'::text)
                                       ->  Bitmap Index Scan on issue_body_trgm  (cost=0.00..13.22 rows=80 width=0) (actual time=0.105..0.105 rows=2001 loops=1)
                                             Index Cond: (body ~~* '%the%'::text)
                           ->  Hash Join  (cost=132.43..1488.06 rows=19990 width=7) (actual time=0.342..36.756 rows=20001 loops=1)
                                 Hash Cond: (c.issue_id = i_1.id)
                                 ->  Seq Scan on comment c  (cost=0.00..1303.03 rows=20000 width=7) (actual time=0.006..34.234 rows=20001 loops=1)
                                       Filter: (body ~~* '%the%'::text)
                                       Rows Removed by Filter: 1
                                 ->  Hash  (cost=107.36..107.36 rows=2005 width=7) (actual time=0.323..0.324 rows=2005 loops=1)
                                       Buckets: 2048  Batches: 1  Memory Usage: 94kB
                                       ->  Index Only Scan using issue_workspace_id_id_key on issue i_1  (cost=0.28..107.36 rows=2005 width=7) (actual time=0.022..0.170 rows=2005 loops=1)
                                             Index Cond: (workspace_id = '1323dfa1-2f47-458b-b5f4-b7de6e0b22d4'::uuid)
                                             Heap Fetches: 0
               ->  Hash  (cost=354.06..354.06 rows=2006 width=1133) (actual time=3.069..3.070 rows=2006 loops=1)
                     Buckets: 2048  Batches: 1  Memory Usage: 2314kB
                     ->  Seq Scan on issue i  (cost=0.00..354.06 rows=2006 width=1133) (actual time=0.009..1.608 rows=2006 loops=1)
 Planning Time: 1.605 ms
 Execution Time: 63.670 ms
(36 rows)

=== q=needle-7731 statement 2 (newest matching comment per issue)
                                                                 QUERY PLAN                                                                  
---------------------------------------------------------------------------------------------------------------------------------------------
 Unique  (cost=129.06..129.07 rows=1 width=371) (actual time=0.250..0.251 rows=2 loops=1)
   ->  Sort  (cost=129.06..129.07 rows=1 width=371) (actual time=0.250..0.250 rows=3 loops=1)
         Sort Key: c.issue_id, c.created_at DESC, e.seq DESC
         Sort Method: quicksort  Memory: 26kB
         ->  Nested Loop  (cost=86.73..129.05 rows=1 width=371) (actual time=0.106..0.226 rows=3 loops=1)
               ->  Bitmap Heap Scan on comment c  (cost=82.37..90.14 rows=1 width=379) (actual time=0.084..0.092 rows=3 loops=1)
                     Recheck Cond: (body ~~* '%needle-7731%'::text)
                     Filter: (issue_id = ANY ('{PL-106,PL-1006,PL-1006}'::text[]))
                     Heap Blocks: exact=3
                     ->  Bitmap Index Scan on comment_body_trgm  (cost=0.00..82.37 rows=2 width=0) (actual time=0.072..0.072 rows=3 loops=1)
                           Index Cond: (body ~~* '%needle-7731%'::text)
               ->  Bitmap Heap Scan on event e  (cost=4.36..38.90 rows=1 width=31) (actual time=0.025..0.043 rows=1 loops=3)
                     Recheck Cond: (issue_id = c.issue_id)
                     Filter: (comment_id = c.id)
                     Rows Removed by Filter: 9
                     Heap Blocks: exact=30
                     ->  Bitmap Index Scan on event_issue  (cost=0.00..4.36 rows=10 width=0) (actual time=0.006..0.006 rows=10 loops=3)
                           Index Cond: (issue_id = c.issue_id)
 Planning Time: 1.401 ms
 Execution Time: 0.314 ms
(20 rows)

```
