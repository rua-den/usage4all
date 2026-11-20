"""claude-usage            -> dashboard + tray (single instance)
claude-usage --hidden     -> same, window hidden (autostart)
claude-usage hook         -> Claude Code UserPromptSubmit hook"""
import sys


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "hook":
        import promptguard
        sys.exit(promptguard.run())
    if len(sys.argv) > 1 and sys.argv[1] == "dashboard":
        import dashboard
        dashboard.run()
        return
    import app
    app.main(hidden="--hidden" in sys.argv)


if __name__ == "__main__":
    main()

