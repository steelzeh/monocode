// Bundle the same Prettier version, parsers and defaults as shared/lib/format.ts.
import { formatWithCursor } from "prettier/standalone";
import * as estree from "prettier/plugins/estree";
import * as babel from "prettier/plugins/babel";
import * as typescript from "prettier/plugins/typescript";
import * as postcss from "prettier/plugins/postcss";
import * as html from "prettier/plugins/html";
import * as markdown from "prettier/plugins/markdown";
import { load as loadYaml } from "js-yaml";
import JSON5 from "json5";
import { parse as parseToml } from "smol-toml";
import ignore from "ignore";
import picomatch from "picomatch";

// Settings files found by monocode-core's project_tools::prettier_config.
type SettingsFile = { name: string; folder: string; text: string };
type Settings = {
  config?: SettingsFile | null;
  ignore?: SettingsFile | null;
  // Nearest folder first.
  editorconfig?: SettingsFile[];
  // Why the project's settings cannot be used without running repo code.
  unusable?: string | null;
};
type Options = Record<string, unknown>;

class Unusable extends Error {}

const slashes = (path: string) => path.replace(/\\/g, "/");
const relativeTo = (folder: string, path: string) =>
  slashes(path).slice(slashes(folder).replace(/\/+$/, "").length + 1);
const listOf = (value: unknown): string[] =>
  Array.isArray(value) ? value : value === undefined ? [] : [value as string];

// js-yaml throws on a file with no content, but for Prettier that means no settings.
const parseYaml = (source: string): unknown =>
  /\S/.test(source.replace(/^\s*#.*$/gm, "")) ? loadYaml(source) : undefined;

function parseConfig({ name, text }: SettingsFile): unknown {
  const source = text.replace(/^\uFEFF/, "");
  if (name === "package.json") return JSON.parse(source).prettier;
  if (name === "package.yaml") return (parseYaml(source) as Options | undefined)?.prettier;
  if (name.endsWith(".json")) return JSON.parse(source);
  if (name.endsWith(".json5")) return JSON5.parse(source);
  if (name.endsWith(".toml")) return parseToml(source);
  return parseYaml(source);
}

// Prettier matches patterns without a slash against the base name only.
function matchesOverride(relative: string, files: unknown, excludeFiles: unknown) {
  const patterns = listOf(files);
  const test = (list: string[], basename: boolean) =>
    list.length > 0 && picomatch(list, { ignore: listOf(excludeFiles), basename, dot: true })(relative);
  return test(patterns.filter(p => !p.includes("/")), true) || test(patterns.filter(p => p.includes("/")), false);
}

function configOptions(file: SettingsFile, path: string): Options {
  let config: unknown;
  try {
    config = parseConfig(file);
  } catch {
    throw new Unusable(`${file.name} could not be read`);
  }
  if (!config) return {};
  if (typeof config === "string") throw new Unusable(`${file.name} points to a shared config package`);
  if (typeof config !== "object" || Array.isArray(config)) throw new Unusable(`${file.name} is not a settings object`);
  const { $schema: _schema, overrides, ...options } = config as Options;
  const relative = relativeTo(file.folder, path);
  for (const override of Array.isArray(overrides) ? overrides : []) {
    if (matchesOverride(relative, override?.files, override?.excludeFiles)) Object.assign(options, override?.options);
  }
  if (listOf(options.plugins).length > 0) throw new Unusable(`${file.name} uses Prettier plugins`);
  delete options.plugins;
  return options;
}

function editorconfigSections(text: string) {
  const sections: { glob: string | null; values: Record<string, string> }[] = [{ glob: null, values: {} }];
  for (const line of text.split(/\r?\n/)) {
    if (/^\s*([#;].*)?$/.test(line)) continue;
    const section = /^\s*\[(([^#;]|\\#|\\;)+)\]\s*([#;].*)?$/.exec(line);
    if (section) {
      sections.push({ glob: section[1], values: {} });
      continue;
    }
    const pair = /^\s*([\w.\-]+)\s*[=:]\s*(.*?)\s*([#;].*)?$/.exec(line);
    if (pair) sections[sections.length - 1].values[pair[1].toLowerCase()] = pair[2];
  }
  return sections;
}

function editorconfigMatches(glob: string, relative: string) {
  const pattern = glob.includes("/") ? glob.replace(/^\//, "") : `**/${glob}`;
  return picomatch(pattern, { dot: true, noextglob: true })(relative);
}

// The same steps as Prettier's resolveConfig: EditorConfig's own rules first,
// then its conversion to Prettier options.
function editorconfigOptions(files: SettingsFile[], path: string): Options {
  const chain: { file: SettingsFile; sections: ReturnType<typeof editorconfigSections> }[] = [];
  for (const file of files) {
    const sections = editorconfigSections(file.text);
    chain.unshift({ file, sections });
    if (sections[0].values.root?.toLowerCase() === "true") break;
  }
  const found: Record<string, string | number> = {};
  for (const { file, sections } of chain) {
    const relative = relativeTo(file.folder, path);
    for (const { glob, values } of sections) {
      if (glob === null || !editorconfigMatches(glob, relative)) continue;
      for (const [key, value] of Object.entries(values)) {
        const lower = value.toLowerCase();
        found[key] = /^\d+$/.test(lower) ? Number(lower) : lower;
      }
    }
  }
  if (found.indent_style === "tab" && found.indent_size === undefined) found.indent_size = "tab";
  if (found.indent_size !== undefined && found.tab_width === undefined && found.indent_size !== "tab") found.tab_width = found.indent_size;
  if (found.indent_size === "tab" && found.tab_width !== undefined) found.indent_size = found.tab_width;
  for (const key of Object.keys(found)) if (found[key] === "unset") delete found[key];

  const options: Options = {};
  if (found.indent_style) options.useTabs = found.indent_style === "tab";
  if (found.indent_size === "tab") options.useTabs = true;
  if (options.useTabs && found.tab_width) options.tabWidth = found.tab_width;
  else if (found.indent_style !== "tab" && found.indent_size !== "tab") options.tabWidth = found.indent_size;
  else if (found.tab_width !== undefined) options.tabWidth = found.tab_width;
  if (found.max_line_length) options.printWidth = found.max_line_length === "off" ? Infinity : found.max_line_length;
  if (found.quote_type === "single") options.singleQuote = true;
  else if (found.quote_type === "double") options.singleQuote = false;
  if (["cr", "crlf", "lf"].includes(found.end_of_line as string)) options.endOfLine = found.end_of_line;
  for (const key of Object.keys(options)) if (options[key] === undefined) delete options[key];
  return options;
}

function isIgnored(file: SettingsFile | null | undefined, path: string) {
  return !!file && ignore({ allowRelativePaths: true }).add(file.text).checkIgnore(relativeTo(file.folder, path)).ignored;
}

globalThis.monocodeFormat = async (path: string, source: string, cursorOffset: number, settingsJson?: string) => {
  if (source.length > 512 * 1024) return "null";
  const ext = path.toLowerCase().split(".").pop();
  const parser = ["js", "jsx", "mjs", "cjs"].includes(ext!) ? "babel"
    : ["ts", "tsx", "mts", "cts"].includes(ext!) ? "typescript"
    : ext === "json" ? "json" : ext === "css" ? "css"
    : ext === "html" || ext === "htm" ? "html"
    : ext === "md" || ext === "markdown" ? "markdown"
    : ext === "mdx" ? "mdx" : null;
  if (!parser) return "null";
  const blocked = (reason: string) => JSON.stringify({ blocked: `Not formatted: ${reason}.` });
  let options: Options;
  try {
    const settings: Settings = settingsJson ? JSON.parse(settingsJson) : {};
    if (isIgnored(settings.ignore, path)) return "null";
    if (settings.unusable) return blocked(settings.unusable);
    options = {
      ...editorconfigOptions(settings.editorconfig ?? [], path),
      ...(settings.config ? configOptions(settings.config, path) : {}),
    };
  } catch (error) {
    return error instanceof Unusable ? blocked(error.message) : "null";
  }
  try {
    return JSON.stringify(await formatWithCursor(source, { parser, ...options, filepath: path, cursorOffset, plugins: [estree, babel, typescript, postcss, html, markdown] }));
  } catch (error) {
    // A syntax error in the file is normal while editing. A rejected setting is not.
    const { name, message } = error as Error;
    return name === "ConfigError" || name === "UndefinedParserError" || /^Invalid /.test(message)
      ? blocked("a Prettier setting for this file has a value that Prettier rejects")
      : "null";
  }
};
