// Prettier's own defaults, plus the two the tree already forced.
//
// semi: false — src/ and ci/ are written without semicolons (5 lines in the
//   whole frontend end in one). `semi: true`, the default, would rewrite every
//   line of every file.
// singleQuote: true — the tree quotes with '...' throughout. The default,
//   double quotes, is the same kind of whole-tree rewrite.
//
// Everything else is deliberately left at the default, because the sources
// already agree with it: 2-space indent, 80 columns (line lengths here peak at
// 77–78), trailing commas in every multi-line list, parens around a single
// arrow parameter. One layout that an editor, an agent and `prettier --check`
// all reach with nothing to look up is the point; these two exceptions are
// paid for because the alternative is churning the tree, not because the
// default is wrong.
export default {
  semi: false,
  singleQuote: true,
}
