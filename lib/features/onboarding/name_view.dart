import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:hoppler/src/rust/api/types.dart';

/// The first thing a new device asks (R0-F1).
///
/// R0-F1 says the person chooses a display name on first launch, and until now
/// nothing asked. The placeholder shipped as the real name, so every device was
/// called "Me" and the pairing screen read "Pairing with Me" — on the one
/// screen whose whole job is saying who is at the other end.
///
/// # Why this is a screen and not a dialog over the app
///
/// The name is not decoration: it is what everyone nearby sees, what a Ping
/// arrives as, and what the other person reads while deciding whether the
/// colours in front of them belong to the friend they are standing with. A
/// device with no chosen name should not be discoverable yet, so this stands in
/// front of everything rather than floating over a list that is already live.
///
/// # The colour is chosen here too
///
/// R0-F1 asks for a name *and* a colour, and until now the core drew one and
/// the screen only showed it. Showing it was the smaller half: the colour is
/// the thing a person recognises their own device by in a list, and one drawn
/// at random is as likely as not to be the one they would not have picked.
///
/// The drawn colour stays the starting selection rather than the row starting
/// empty — a screen that refuses to continue until two things are chosen is
/// twice as long as it needs to be, and the random colour was already good
/// enough to ship.
///
/// # What it does not do
///
/// No skip. A name is one field and takes a moment, and "Me" is precisely the
/// answer that makes the ceremony screen unreadable — offering to keep it would
/// be offering the bug back. What it does allow is a name that is *not*
/// unique: nothing here is an account, two people called Sam are two people
/// called Sam, and the ceremony's colours are what actually distinguishes them.
class NameView extends StatefulWidget {
  const NameView({
    super.key,
    required this.colour,
    required this.palette,
    required this.onChosen,
  });

  /// Matches `MAX_PERSONA_NAME_LEN` in `identity/mod.rs`, which truncates
  /// anything longer. Enforced here so a long name is refused while it is being
  /// typed rather than silently shortened after it is chosen — a person who
  /// typed a name and got a different one back has been told nothing.
  ///
  /// **Bytes, not characters.** Flutter's `maxLength` counts code units, and
  /// this limit is a byte count in Rust, so the two disagree for every name
  /// outside ASCII — sixty-four emoji are 64 to `maxLength` and 256 to the
  /// core, which would truncate them after submission. That is the exact
  /// failure the paragraph above claims to prevent, so it is counted properly
  /// by [_ByteLimit] rather than approximated.
  static const maxNameBytes = 64;

  /// The colour the core drew for this device, and the starting selection.
  final Color colour;

  /// The colours on offer, from the core so that nothing has to keep a second
  /// copy of the palette in step. Each carries its name, which is what labels
  /// the swatch for anyone who cannot tell two of them apart.
  final List<PersonaColourDto> palette;

  /// Called with a trimmed, non-empty name and the chosen colour, packed
  /// 0xRRGGBB as the core stores it.
  final Future<void> Function(String name, int colour) onChosen;

  @override
  State<NameView> createState() => _NameViewState();
}

class _NameViewState extends State<NameView> {
  final _controller = TextEditingController();
  bool _saving = false;
  String? _failed;
  /// The core's draw, as a starting point only.
  ///
  /// A `late` initializer rather than `initState`, which is the same thing
  /// later: it runs on first *read*, which is in `build`, so `widget` is long
  /// since set. Kept as one line because that is where the field's meaning is.
  ///
  /// Not kept in step with [NameView.colour] afterwards, and that is deliberate
  /// rather than overlooked: a `didUpdateWidget` here would silently discard
  /// somebody's pick if the parent ever rebuilt with a new draw. It cannot
  /// today — the persona only changes once a name is chosen, and that is the
  /// moment this screen is replaced — but "the parent overwrote your choice"
  /// is a worse failure than a stale prop, so the choice wins if they ever
  /// disagree.
  late int _colour = widget.colour.toARGB32() & 0x00ffffff;

  bool get _usable => _controller.text.trim().isNotEmpty;

  Future<void> _submit() async {
    final name = _controller.text.trim();
    // The `_saving` half is belt-and-braces and no test can tell: while saving,
    // the button's `onPressed` is null and the field is disabled, so neither
    // door is open. Kept because both of those are widget state a later change
    // could quietly alter, and noted so the surviving mutant reads as a
    // deliberate second lock rather than a gap.
    if (name.isEmpty || _saving) return;
    setState(() {
      _saving = true;
      _failed = null;
    });
    try {
      await widget.onChosen(name, _colour);
    } catch (e, stack) {
      // Stays on this screen. The core refuses the name when it cannot store
      // it, and continuing into the app would mean discovering as a name the
      // next launch will not have.
      //
      // The exception goes to the log, not the screen. What reaches a person
      // standing here holding a phone has to be something they can act on, and
      // a store error is not — while the detail is exactly what is wanted in a
      // bug report.
      debugPrint('choosing a name failed: $e\n$stack');
      if (mounted) {
        setState(() {
          _saving = false;
          _failed = 'That name could not be saved. Try again.';
        });
      }
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Scaffold(
      body: SafeArea(
        child: LayoutBuilder(
          builder: (context, constraints) => SingleChildScrollView(
            padding: const EdgeInsets.all(24),
            child: ConstrainedBox(
              constraints: BoxConstraints(
                minHeight: constraints.maxHeight - 48,
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  CircleAvatar(
                    radius: 32,
                    backgroundColor: Color(0xFF000000 | _colour),
                  ),
                  const SizedBox(height: 24),
                  Text(
                    'What should people call you?',
                    style: theme.textTheme.headlineSmall,
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'This is what everyone nearby sees. No account, no email — '
                    'it stays on this phone until you meet someone.',
                    style: theme.textTheme.bodyMedium,
                    textAlign: TextAlign.center,
                  ),
                  const SizedBox(height: 24),
                  TextField(
                    controller: _controller,
                    autofocus: true,
                    inputFormatters: const [_ByteLimit(NameView.maxNameBytes)],
                    // Counted in the same units the limit is in. A counter
                    // disagreeing with the thing it counts is worse than none.
                    buildCounter:
                        (
                          context, {
                          required currentLength,
                          required maxLength,
                          required isFocused,
                        }) => Text(
                          '${utf8.encode(_controller.text).length}'
                          '/${NameView.maxNameBytes}',
                          style: theme.textTheme.bodySmall,
                        ),
                    textInputAction: TextInputAction.done,
                    enabled: !_saving,
                    decoration: const InputDecoration(
                      labelText: 'Your name',
                      border: OutlineInputBorder(),
                    ),
                    onChanged: (_) => setState(() {}),
                    onSubmitted: (_) => _submit(),
                  ),
                  const SizedBox(height: 24),
                  // Wrap, not Row: eight swatches at a large text scale on a
                  // narrow phone overflow a Row, and an overflow here is a
                  // colour nobody can reach.
                  Wrap(
                    spacing: 12,
                    runSpacing: 12,
                    alignment: WrapAlignment.center,
                    children: [
                      for (final c in widget.palette)
                        _Swatch(
                          name: c.name,
                          colour: c.value,
                          selected: c.value == _colour,
                          onPick: _saving
                              ? null
                              : () => setState(() => _colour = c.value),
                        ),
                    ],
                  ),
                  if (_failed != null) ...[
                    const SizedBox(height: 8),
                    Text(
                      _failed!,
                      style: theme.textTheme.bodyMedium?.copyWith(
                        color: theme.colorScheme.error,
                      ),
                    ),
                  ],
                  const SizedBox(height: 16),
                  FilledButton(
                    onPressed: _usable && !_saving ? _submit : null,
                    child: Text(_saving ? 'Saving…' : 'Continue'),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Refuses an edit that would put the name over [NameView.maxNameBytes] *bytes*.
///
/// Rejects rather than truncates: cutting the value mid-edit can slice a
/// multi-byte character in half, and refusing the keystroke is what a person
/// reading a full counter expects anyway.
class _ByteLimit extends TextInputFormatter {
  const _ByteLimit(this.maxBytes);

  final int maxBytes;

  @override
  TextEditingValue formatEditUpdate(
    TextEditingValue oldValue,
    TextEditingValue newValue,
  ) => utf8.encode(newValue.text).length > maxBytes ? oldValue : newValue;
}

/// One colour to pick, as a circle.
///
/// # Why the selected one also has a tick
///
/// A ring in the theme's accent colour is the obvious way to show which is
/// chosen, and it is the wrong one on its own: this is a row of colours, the
/// people most likely to be helped by a clear indicator are the ones who cannot
/// separate two of these, and "the one with the differently-coloured outline"
/// asks them to do the thing they cannot. The tick is a shape, so it survives
/// any amount of colour blindness and a greyscale screenshot.
class _Swatch extends StatelessWidget {
  const _Swatch({
    required this.name,
    required this.colour,
    required this.selected,
    required this.onPick,
  });

  final String name;
  final int colour;
  final bool selected;
  final VoidCallback? onPick;

  @override
  Widget build(BuildContext context) {
    final fill = Color(0xFF000000 | colour);
    // Against the swatch, not the page. Half of this palette is light enough
    // that a white tick on it is invisible, which would put the shape back to
    // being no shape at all.
    final tick = ThemeData.estimateBrightnessForColor(fill) == Brightness.dark
        ? Colors.white
        : Colors.black;
    return Semantics(
      label: name,
      button: true,
      selected: selected,
      // The tick and ring are already in the tree; without this the reader
      // announces the name and leaves out the only part that says which is
      // chosen.
      child: InkWell(
        onTap: onPick,
        customBorder: const CircleBorder(),
        child: Container(
          width: 44,
          height: 44,
          decoration: BoxDecoration(
            color: fill,
            shape: BoxShape.circle,
            border: selected
                ? Border.all(color: Theme.of(context).colorScheme.onSurface, width: 3)
                : null,
          ),
          child: selected ? Icon(Icons.check, size: 22, color: tick) : null,
        ),
      ),
    );
  }
}
