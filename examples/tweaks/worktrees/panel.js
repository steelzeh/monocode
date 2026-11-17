// A row for each working copy, with one button to show them all.
let copies = [];
let focused = null;
let problem = null;

const load = () =>
  tweak.app("worktrees.list", {}, (result, error) => {
    problem = error || null;
    if (result) {
      copies = result.worktrees.filter((copy) => !copy.missing);
      focused = result.focused;
    }
  });

tweak.onRefresh(load);

const select = (path) => tweak.app("worktrees.select", path ? { path } : {}, load);

const name = (copy) => {
  const label = copy.branch || copy.path.split("/").pop();
  return copy.submodule ? `${copy.submodule}: ${label}` : label;
};

return () =>
  visual.column([
    visual.text(tweak.context.project, { size: "subtitle" }),
    ...(problem ? [visual.text(problem, { color: "negative" })] : []),
    visual.button("All working copies", () => select(null), {
      disabled: focused === null,
    }),
    ...copies.map((copy) =>
      visual.button(copy.main ? `${name(copy)} (main)` : name(copy), () => select(copy.path), {
        disabled: copy.path === focused,
      }),
    ),
  ]);
