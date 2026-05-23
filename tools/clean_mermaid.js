#!/usr/bin/env node
const fs = require('fs');
const path = require('path');

const src = path.resolve(process.cwd(), 'UML.md');
const outDir = path.resolve(process.cwd(), 'design_docs', 'diagrams');
const out = path.join(outDir, 'uml_clean.mmd');

if (!fs.existsSync(src)) {
  console.error('UML.md not found in project root');
  process.exit(1);
}

const txt = fs.readFileSync(src, 'utf8');
const m = txt.match(/```mermaid([\s\S]*?)```/);
if (!m) {
  console.error('No mermaid block found in UML.md');
  process.exit(1);
}

let content = m[1];

// sanitize common problematic tokens for mermaid parsers
content = content
  .replace(/`/g, '')             // remove backticks
  .replace(/</g, '_')            // replace angle brackets
  .replace(/>/g, '_')
  .replace(/~/g, '_')            // replace tildes used by auto-uml
  .replace(/[\\/]/g, '_')      // replace slashes
  .replace(/namespace\s+/g, '') // remove namespace keyword
  .replace(/&/g, 'and')          // avoid stray ampersands
  ;

// ensure output directory exists
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(out, '```mermaid\n' + content.trim() + '\n```');
console.log('Wrote cleaned mermaid to', out);
