# Pull requests prototype

A throwaway prototype of a Pull requests view, against mock data only (PL-57). Nothing here is wired to production code. Punchlist only links out to GitHub here; reviewing and merging inside Punchlist is later work. Gannon picks one layout; then the wiring is cut into slices.

Open `index.html` in a browser. No build step and no server. `?variant=page|tab|queue` deep-links a layout.

| Layout | Key | Shape | Reached from the list and the board |
| --- | --- | --- | --- |
| Page | `1` | A screen of its own: one table, sectioned by what a person has to do (waits on a merge, in review, draft, names no issue) | A Pull requests item next to Issues in the app header; `g` `p` |
| List tab | `2` | A third layout of the issue list, grouped by the issue's status, closest to Done first, with a banner counting merges waiting | A third button in the List / Board switcher; `3` |
| Merge queue | `3` | A panel over the list or board, with lanes by what happens next | A Pull requests button with a "to merge" count in the top bar; the board's Merging column header |

Every layout shows each open or draft pull request with its number, title, branch, the issue it names (or "names no issue", as `pl pr unlinked` does), the issue's status, checks passed of total, open review threads, merge state and an **Open on GitHub** link to `https://github.com/<owner>/<repo>/pull/<n>`. An issue in Merging reads as waiting on a merge on GitHub, since Merging → Done moves on `pr_merged`, with a Merge on GitHub link. A panel at the top of each layout shows how a person reaches the view from the list and the board, and how an issue row and the issue page's pull request section link to GitHub.

Rows and links are in the Tab order. `Enter` on a row opens its issue (a toast in the prototype), or the pull request on GitHub when it names no issue; `Enter` on a link opens GitHub.

Styles load `../issue-list/styles.css` and use its theme variables only. Mock data lives in `data.js`.
