// Independent oracle: imports the pinned upstream implementation, never Rust output.
import * as server from "./upstream/typescript/src/server/index.ts";
import * as app from "./upstream/typescript/src/app/index.ts";
import fs from "node:fs";
import readline from "node:readline";
import ts from "typescript";
const modules = {server, app};
export function validate({module = "server", schema, value, form}) {
  if (schema === "OpenAIFormContentSchema") {
    try {
      const checked = server.OpenAIFormSchema.parse(form);
      const result = server.createOpenAIFormContentSchema(checked).safeParse(value);
      return result.success ? {ok:true, value:result.data} : {ok:false, error:"validation_error"};
    } catch { return {ok:false, error:"validation_error"}; }
  }
  const validator = modules[module]?.[schema];
  if (!validator?.safeParse) return {ok:false, error:"unknown_schema", schema};
  const result = validator.safeParse(value);
  return result.success ? {ok:true, value:result.data} : {ok:false, error:"validation_error"};
}
function inventory() {
  const exports = {};
  for (const module of Object.keys(modules)) {
    const path = new URL(`./upstream/typescript/src/${module}/index.ts`, import.meta.url);
    const ast = ts.createSourceFile(path.pathname, fs.readFileSync(path, "utf8"), ts.ScriptTarget.Latest, true);
    exports[module] = ast.statements.flatMap(statement =>
      ts.isExportDeclaration(statement) && statement.exportClause && ts.isNamedExports(statement.exportClause)
        ? statement.exportClause.elements.map(element => ({name:element.name.text, typeOnly:statement.isTypeOnly || element.isTypeOnly}))
        : []).sort((a,b)=>a.name.localeCompare(b.name));
  }
  return exports;
}
if (process.argv.includes("--inventory")) {
  console.log(JSON.stringify(inventory()));
} else if (process.argv.includes("--fixtures")) {
  const fixtures = JSON.parse(fs.readFileSync(new URL("./fixtures.json", import.meta.url), "utf8"));
  const failed = [];
  for (const fixture of fixtures) {
    const actual = validate(fixture);
    if (actual.ok !== fixture.expected) failed.push({name:fixture.name, expected:fixture.expected, actual});
  }
  console.log(JSON.stringify({cases:fixtures.length, failed}));
  process.exitCode = failed.length ? 1 : 0;
} else {
  for await (const line of readline.createInterface({input:process.stdin, crlfDelay:Infinity})) {
    try { console.log(JSON.stringify(validate(JSON.parse(line)))); }
    catch (error) { console.log(JSON.stringify({ok:false,error:String(error)})); }
  }
}
