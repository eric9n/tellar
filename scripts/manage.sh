#!/bin/bash

# Tellar Service Manager
# A utility script to manage the Tellar systemd user service.

SERVICE_NAME="tellar"
SERVICE_FILE="scripts/tellar.service"
SYSTEMD_USER_DIR="$HOME/.config/systemd/user"
TARGET_SERVICE_PATH="$SYSTEMD_USER_DIR/$SERVICE_NAME.service"

show_usage() {
    echo "Usage: $0 {setup|start|stop|restart|status|logs}"
    exit 1
}

case "$1" in
    setup)
        GUILD_PATH="${2:-$HOME/.tellar}"
        
        # Convert to absolute path
        ABS_GUILD_PATH=$(realpath -m "$GUILD_PATH")

        echo "🕯️ Setting up Tellar service with guild: $ABS_GUILD_PATH"
        mkdir -p "$SYSTEMD_USER_DIR"
        
        # Inject the path into the service file using sed
        sed "s|{{GUILD_PATH}}|$ABS_GUILD_PATH|g" "$SERVICE_FILE" > "$TARGET_SERVICE_PATH"
        
        echo "✅ Service file created at $TARGET_SERVICE_PATH"
        
        systemctl --user daemon-reload
        systemctl --user enable $SERVICE_NAME
        
        echo "🔧 Enabling linger for $USER..."
        loginctl enable-linger $USER
        
        echo "🚀 Setup complete. Run '$0 start' to begin stewardship."
        ;;
    start)
        echo "🕯️ Starting Tellar..."
        systemctl --user start $SERVICE_NAME
        ;;
    stop)
        echo "😴 Stopping Tellar..."
        systemctl --user stop $SERVICE_NAME
        ;;
    restart)
        echo "🔄 Restarting Tellar..."
        systemctl --user restart $SERVICE_NAME
        ;;
    status)
        systemctl --user status $SERVICE_NAME
        ;;
    logs)
        journalctl --user -u $SERVICE_NAME -f
        ;;
    *)
        show_usage
        ;;
esac
