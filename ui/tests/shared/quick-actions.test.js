import { describe, it, expect } from 'vitest';
import { classifyText } from '../../js/shared/quick-actions.js';

describe('classifyText', () => {
  // --- Code detection ---

  it('detects Python code', () => {
    const types = classifyText('def hello():\n    print("hi")');
    expect(types).toContain('code');
  });

  it('detects JavaScript code', () => {
    const types = classifyText('const x = () => {\n  return 42;\n};');
    expect(types).toContain('code');
  });

  it('detects Rust code', () => {
    const types = classifyText('pub fn main() {\n    println!("hello");\n}');
    expect(types).toContain('code');
  });

  it('detects code with import statement', () => {
    const types = classifyText('import os\nimport sys');
    expect(types).toContain('code');
  });

  it('detects code with class keyword', () => {
    const types = classifyText('class MyClass {\n  constructor() {}\n}');
    expect(types).toContain('code');
  });

  it('detects code with Rust attributes', () => {
    const types = classifyText('#[derive(Debug)]\nstruct Foo {}');
    expect(types).toContain('code');
  });

  it('detects code with decorators', () => {
    const types = classifyText('@app.route("/api")\ndef handler():');
    expect(types).toContain('code');
  });

  // --- Error detection ---

  it('detects stack traces', () => {
    const types = classifyText('Error: something failed\n    at Object.<anonymous> (file.js:10:5)');
    expect(types).toContain('error');
  });

  it('detects Python tracebacks', () => {
    const types = classifyText('Traceback (most recent call last):\n  File "app.py", line 42');
    expect(types).toContain('error');
  });

  it('detects Exception keyword', () => {
    const types = classifyText('java.lang.NullPointerException: cannot invoke method\n    at com.example.Main.run(Main.java:42)');
    expect(types).toContain('error');
  });

  // --- JSON detection ---

  it('detects JSON objects', () => {
    const types = classifyText('{"name": "test", "value": 42}');
    expect(types).toContain('json');
  });

  it('detects JSON arrays', () => {
    const types = classifyText('[1, 2, 3]');
    expect(types).toContain('json');
  });

  it('detects YAML-like key-value pairs', () => {
    const types = classifyText('name: test\nversion: 1.0\nauthor: me');
    expect(types).toContain('json');
  });

  // --- URL detection ---

  it('detects URLs', () => {
    const types = classifyText('https://example.com/path');
    expect(types).toContain('url');
  });

  it('detects http URLs', () => {
    const types = classifyText('http://localhost:3000');
    expect(types).toContain('url');
  });

  // --- Math detection ---

  it('detects math expressions', () => {
    const types = classifyText('2 + 3 * 4');
    expect(types).toContain('math');
  });

  it('detects expressions with division', () => {
    const types = classifyText('100 / 5 = 20');
    expect(types).toContain('math');
  });

  // --- Number detection ---

  it('detects plain numbers', () => {
    const types = classifyText('42');
    expect(types).toContain('number');
  });

  it('detects numbers with currency symbols', () => {
    const types = classifyText('$1,234.56');
    expect(types).toContain('number');
  });

  it('detects percentages', () => {
    const types = classifyText('99.5%');
    expect(types).toContain('number');
  });

  // --- Prose detection ---

  it('returns prose for plain text', () => {
    const types = classifyText('The quick brown fox jumps over the lazy dog');
    expect(types).toContain('prose');
    expect(types).not.toContain('code');
  });

  it('returns prose for simple sentences', () => {
    const types = classifyText('Hello, how are you doing today?');
    expect(types).toContain('prose');
  });

  // --- Folder plan detection ---

  it('detects folder plan text', () => {
    const types = classifyText("Here's what I would organize in the folder. The plan includes moving duplicates.");
    expect(types).toContain('folder_plan');
  });

  // --- Multiple types ---

  it('can return multiple types for code with errors', () => {
    const types = classifyText('function foo() {\n  throw new Error("fail");\n}');
    expect(types).toContain('code');
    expect(types).toContain('error');
  });
});
