module.exports = grammar({
  name: 'ezra_asm',

  extras: _ => [/[ \t\r]/],

  rules: {
    source_file: $ => repeat(choice($.line, $.comment_line, '\n')),

    line: $ => choice(
      seq(
        $.label,
        optional(choice($.directive, $.instruction)),
        repeat(choice($.operand, $.punctuation)),
        optional($.comment),
        '\n',
      ),
      seq(
        choice($.directive, $.instruction),
        repeat(choice($.operand, $.punctuation)),
        optional($.comment),
        '\n',
      ),
    ),
    comment_line: $ => seq($.comment, '\n'),

    label: _ => token(/[A-Za-z_.][A-Za-z0-9_.]*:/),
    directive: _ => token(/[%#.][A-Za-z_][A-Za-z0-9_.]*/),
    instruction: _ => token(/[A-Za-z_][A-Za-z0-9_.]*/),
    operand: $ => choice($.number, $.string, $.character, $.identifier),
    punctuation: _ => choice(',', '(', ')', '[', ']', '+', '-', '*', '/', '&', '|', '^', '~', '='),

    number: _ => token(choice(
      /0[xX][0-9A-Fa-f]+/,
      /0[bB][01]+/,
      /\$[0-9A-Fa-f]+/,
      /[0-9A-Fa-f]+[hH]/,
      /%[01]+/,
      /[0-9]+/,
    )),
    string: _ => /"(\\.|[^"\\\n])*"/,
    character: _ => /'(\\.|[^'\\\n])'/,
    identifier: _ => /[A-Za-z_.][A-Za-z0-9_.]*/,
    comment: _ => token(choice(seq(';', /.*/), seq('//', /.*/))),
  },
});
