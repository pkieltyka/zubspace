// Read tracing's text and JSON formats without treating escaped JSON message
// contents as a second log envelope. One-entry cache bounds retained memory.
let previousLine = null;
let previousRecord = null;
export function serverLogRecord(line) {
  if (line === previousLine) return previousRecord;
  let fields = null;
  if (line.trimStart().startsWith("{") && line.includes('"fields"')) {
    const record = JSON.parse(line);
    if (!record.fields || typeof record.fields !== "object" || Array.isArray(record.fields)) throw new Error("malformed structured server log fields");
    fields = record.fields;
  }
  const message = fields ? (typeof fields.message === "string" ? fields.message : "") : line;
  previousLine = line;
  previousRecord = { fields, message };
  return previousRecord;
}
export function serverLogField(line, name) {
  const { fields } = serverLogRecord(line);
  return fields && Object.hasOwn(fields, name) ? fields[name] : undefined;
}
export function serverLogMessage(line) { return serverLogRecord(line).message; }
