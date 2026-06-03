#!/bin/bash
# Run SignalFlow Bot in tmux background session

SESSION_NAME="signalflow-bot"

# Kill existing session if exists
tmux kill-session -t $SESSION_NAME 2>/dev/null

# Create new tmux session and run bot
tmux new-session -d -s $SESSION_NAME -c /root/signalflow-bot

# Send the start command
tmux send-keys -t $SESSION_NAME "./start.sh" Enter

echo "✅ Bot started in tmux session: $SESSION_NAME"
echo ""
echo "Commands:"
echo "  tmux attach -t $SESSION_NAME    # Attach to session"
echo "  tmux kill-session -t $SESSION_NAME  # Stop bot"
echo "  tmux ls                         # List sessions"
