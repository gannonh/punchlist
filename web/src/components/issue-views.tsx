// The two approved layouts (prototypes/issue-list/): the grouped list and the board.
import { memo, useLayoutEffect } from "react";
import { Lock } from "lucide-react";
import { cn } from "cn";
import {
  Assignee,
  Gates,
  Labels,
  LockBadge,
  PullRequest,
  Run,
  StatusDot,
  TransitionNote,
} from "@/components/atoms";
import {
  agentName,
  peopleOnly,
  rowView,
  type Group,
  type IssueRow,
  type Transition,
  type Transitions,
} from "@/model/issues";

/**
 * Records a `pl:render` measure for each commit of the list or the board, from the start of the view's render to
 * its DOM being updated, so a live check reads `performance.getEntriesByName('pl:render')`.
 */
function useRenderMeasure(layout: "list" | "board", groups: Group[]) {
  const start = `pl:render-start:${layout}`;
  performance.mark(start);
  useLayoutEffect(() => {
    const rows = groups.reduce((n, group) => n + group.rows.length, 0);
    performance.measure("pl:render", { start, detail: { layout, rows } });
  });
}

interface RowProps {
  row: IssueRow;
  selected: boolean;
  transition: Transition | undefined;
  now: Date;
  onPick: (id: string) => void;
  onDismiss: (id: string) => void;
}

function GroupHead({ group }: { group: Group }) {
  return (
    <div
      className="flex flex-wrap items-center gap-2 text-body font-semibold"
      data-status={group.status.key}
    >
      <StatusDot status={group.status.key} />
      <span>{group.status.name}</span>
      <span className="font-medium text-ink-3">{group.rows.length}</span>
      <LockBadge status={group.status} />
      {group.status.dispatch && (
        <span
          className="text-meta font-medium text-ink-3"
          title={`Issues in this status are dispatched to ${agentName(group.status.dispatch)}`}
        >
          dispatches {agentName(group.status.dispatch)}
        </span>
      )}
    </div>
  );
}

const ListRow = memo(function ListRow({
  row,
  selected,
  transition,
  now,
  onPick,
  onDismiss,
}: RowProps) {
  const view = rowView(row, now);
  return (
    <div
      className="list-row cursor-default items-center border-t px-2.5 py-2 outline-none first:border-t-0 hover:bg-stone-50"
      role="option"
      data-issue={view.id}
      data-status={view.status}
      aria-selected={selected}
      tabIndex={-1}
      onClick={() => {
        onPick(view.id);
      }}
    >
      <span className="font-mono text-meta text-ink-3">{view.id}</span>
      <span className="title truncate font-medium max-lg:whitespace-normal">
        {view.title}
        <Labels labels={view.labels} />
      </span>
      <Assignee view={view.assignee} />
      <span className="meta">
        <Run view={view.run} />
        <PullRequest view={view.pullRequest} />
        <Gates view={view.gates} />
      </span>
      {transition && (
        <TransitionNote
          transition={transition}
          onDismiss={() => {
            onDismiss(view.id);
          }}
        />
      )}
    </div>
  );
});

const Card = memo(function Card({
  row,
  selected,
  transition,
  now,
  onPick,
  onDismiss,
}: RowProps) {
  const view = rowView(row, now);
  return (
    <article
      className="grid cursor-default gap-1.5 rounded-md border bg-card px-2.5 py-2 outline-none hover:border-stone-300"
      role="option"
      data-issue={view.id}
      data-status={view.status}
      aria-selected={selected}
      tabIndex={-1}
      onClick={() => {
        onPick(view.id);
      }}
    >
      <div className="flex items-center justify-between gap-2 text-meta text-ink-3">
        <span className="font-mono">{view.id}</span>
        <Assignee view={view.assignee} short />
      </div>
      <div className="title leading-snug font-medium wrap-anywhere">
        {view.title}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Run view={view.run} />
        <PullRequest view={view.pullRequest} />
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Gates view={view.gates} />
      </div>
      {transition && (
        <TransitionNote
          transition={transition}
          onDismiss={() => {
            onDismiss(view.id);
          }}
        />
      )}
    </article>
  );
});

interface ViewProps {
  groups: Group[];
  selected: string | null;
  transitions: Transitions;
  now: Date;
  onPick: (id: string) => void;
  onDismiss: (id: string) => void;
}

export function ListView({
  groups,
  selected,
  transitions,
  now,
  onPick,
  onDismiss,
}: ViewProps) {
  useRenderMeasure("list", groups);
  return (
    <div data-layout="list">
      {groups
        .filter((group) => group.rows.length > 0)
        .map((group) => (
          <section
            key={group.status.key}
            className="mb-3.5"
            data-group={group.status.key}
          >
            <div className="px-2 py-1.5">
              <GroupHead group={group} />
            </div>
            <div
              className="overflow-hidden rounded-lg border bg-card"
              role="listbox"
              aria-label={group.status.name}
            >
              {group.rows.map((row) => (
                <ListRow
                  key={row.issue.id}
                  row={row}
                  selected={row.issue.id === selected}
                  transition={transitions[row.issue.id]}
                  now={now}
                  onPick={onPick}
                  onDismiss={onDismiss}
                />
              ))}
            </div>
          </section>
        ))}
    </div>
  );
}

export function BoardView({
  column,
  onColumn,
  ...props
}: ViewProps & { column: string; onColumn: (key: string) => void }) {
  const { groups, selected, transitions, now, onPick, onDismiss } = props;
  useRenderMeasure("board", groups);
  return (
    <div data-layout="board">
      {/* On a narrow screen the board shows one column at a time, chosen from this strip. */}
      <div
        className="mb-2.5 flex flex-wrap gap-1.5 md:hidden"
        role="tablist"
        aria-label="Status columns"
      >
        {groups.map((group) => {
          const current = group.status.key === column;
          return (
            <button
              key={group.status.key}
              type="button"
              role="tab"
              aria-selected={current}
              data-column={group.status.key}
              className={cn(
                "inline-flex cursor-pointer items-center gap-1.5 rounded-full border px-2.25 py-0.75 text-meta",
                current
                  ? "border-foreground bg-foreground text-card"
                  : "bg-card",
              )}
              onClick={() => {
                onColumn(group.status.key);
              }}
            >
              <StatusDot status={group.status.key} />
              {group.status.name}
              <span className="opacity-70">{group.rows.length}</span>
              {peopleOnly(group.status) && (
                <Lock
                  className={cn(
                    "size-2.5",
                    current ? "text-warn-line" : "text-warn",
                  )}
                  aria-label="people only"
                />
              )}
            </button>
          );
        })}
      </div>
      <div className="flex items-start gap-3 overflow-x-auto pb-2 max-md:block max-md:overflow-visible">
        {groups.map((group) => {
          const current = group.status.key === column;
          return (
            <section
              key={group.status.key}
              className={cn(
                "flex max-h-[calc(100vh-150px)] flex-[0_0_272px] flex-col rounded-lg border bg-column",
                current ? "border-primary-line max-md:flex" : "max-md:hidden",
                "max-md:max-h-none",
              )}
              data-column-section={group.status.key}
              aria-current={current}
            >
              <div className="border-b px-2.5 py-2">
                <GroupHead group={group} />
              </div>
              <div
                className="flex flex-col gap-2 overflow-y-auto p-2"
                role="listbox"
                aria-label={group.status.name}
              >
                {group.rows.length > 0 ? (
                  group.rows.map((row) => (
                    <Card
                      key={row.issue.id}
                      row={row}
                      selected={row.issue.id === selected}
                      transition={transitions[row.issue.id]}
                      now={now}
                      onPick={onPick}
                      onDismiss={onDismiss}
                    />
                  ))
                ) : (
                  <div className="rounded-md border border-dashed p-2.5 text-center text-meta text-ink-3">
                    No issues
                  </div>
                )}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}
