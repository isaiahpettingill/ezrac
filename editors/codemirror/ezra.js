import {StreamLanguage} from "@codemirror/language";

const keywords = /^(?:alias|align|as|asm|break|bytes|cast|clobber|const|continue|cstr|else|embed|entry|execute|extern|file|fn|global|if|import|in|inline|interrupt|layout|let|load|loop|mmio|naked|out|port|ptr|pub|read|region|repeat|reserved|return|section|stack|struct|symbol|text|volatile|while|write)\b/;
const types = /^(?:u8|i8|u16|i16|u24|i24)\b/;
const atoms = /^(?:true|false)\b/;
const number = /^(?:0x[0-9A-Fa-f]+|0b[01]+|[0-9]+)(?:u8|i8|u16|i16|u24|i24)?\b/;

export const ezraLanguage = StreamLanguage.define({
  name: "ezra",
  token(stream) {
    if (stream.match("//")) {
      stream.skipToEnd();
      return "comment";
    }

    if (stream.match('"')) {
      let escaped = false;
      while (!stream.eol()) {
        const ch = stream.next();
        if (ch === '"' && !escaped) break;
        escaped = ch === "\\" && !escaped;
        if (ch !== "\\") escaped = false;
      }
      return "string";
    }

    if (stream.match(/^'(?:\\[n0t\\'"]|[^'\\])'/)) return "character";
    if (stream.match(number)) return "number";
    if (stream.match(atoms)) return "atom";
    if (stream.match(types)) return "type";
    if (stream.match(keywords)) return "keyword";
    if (stream.match(/^(?:->|\.\.|<<=|>>=|==|!=|<=|>=|&&|\|\||<<|>>|[-+*/%&|^~!=<>]=?)/)) return "operator";

    stream.next();
    return null;
  },
  languageData: {
    commentTokens: {line: "//"}
  }
});
