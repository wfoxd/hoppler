import 'package:flutter/material.dart';

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
/// # What it does not do
///
/// No skip. A name is one field and takes a moment, and "Me" is precisely the
/// answer that makes the ceremony screen unreadable — offering to keep it would
/// be offering the bug back. What it does allow is a name that is *not*
/// unique: nothing here is an account, two people called Sam are two people
/// called Sam, and the ceremony's colours are what actually distinguishes them.
class NameView extends StatefulWidget {
  const NameView({super.key, required this.colour, required this.onChosen});

  /// Matches `MAX_PERSONA_NAME_LEN` in `identity/mod.rs`, which truncates
  /// anything longer. Enforced here so a long name is refused while it is being
  /// typed rather than silently shortened after it is chosen — a person who
  /// typed a name and got a different one back has been told nothing.
  static const maxNameBytes = 64;

  /// The colour the core drew for this device. Shown, not chosen: R0-F1 asks
  /// for a colour too, and offering one here is a small addition — until then
  /// this at least stops it being a surprise the first time they see it.
  final Color colour;

  /// Called with a trimmed, non-empty name.
  final Future<void> Function(String name) onChosen;

  @override
  State<NameView> createState() => _NameViewState();
}

class _NameViewState extends State<NameView> {
  final _controller = TextEditingController();
  bool _saving = false;
  String? _failed;

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
      await widget.onChosen(name);
    } catch (e) {
      // Stays on this screen. The core refuses the name when it cannot store
      // it, and continuing into the app would mean discovering as a name the
      // next launch will not have.
      if (mounted) {
        setState(() {
          _saving = false;
          _failed = 'That name could not be saved. $e';
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
                  CircleAvatar(radius: 32, backgroundColor: widget.colour),
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
                    maxLength: NameView.maxNameBytes,
                    textInputAction: TextInputAction.done,
                    enabled: !_saving,
                    decoration: const InputDecoration(
                      labelText: 'Your name',
                      border: OutlineInputBorder(),
                    ),
                    onChanged: (_) => setState(() {}),
                    onSubmitted: (_) => _submit(),
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
