// Hint only — never touches rc files on install. Binding is explicit:
//   caproom setup
try {
  process.stdout.write(
    'caproom: optional next step — run "caproom setup" to bind headroom warnings to your shells (never modifies rc files on install)\n'
  );
} catch (_) { /* never block installs */ }
