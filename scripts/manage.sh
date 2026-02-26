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
        echo "🕯️ Setting up Tellar service..."
        mkdir -p "$SYSTEMD_USER_DIR"
        cp "$SERVICE_FILE" "$TARGET_SERVICE_PATH"
        echo "✅ Service file copied to $TARGET_SERVICE_PATH"
        
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
