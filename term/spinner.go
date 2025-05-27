package term

import (
	"strings"
	"time"

	"atomicgo.dev/cursor"
)

var (
	Frames1 = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}
	Frames2 = []string{"-", "\\", "|", "/"}
	Frames3 = []string{"◜", "◠", "◝", "◞", "◡", "◟"}
)

// spinner is a simple animation for terminal.
type spinner struct {
	// frames is the list of frames to use for the animation.
	frames []string
	// interval is the interval to use for the animation.
	interval time.Duration
	// index is the current index of the frames.
	index int
	// suffix is the string behind animation.
	suffix string
	// ticker is the ticker used for the animation.
	ticker *time.Ticker
}

// NewSpinner returns a new spinner.
func NewCustomSpinner(frames []string, interval time.Duration) *spinner {
	return &spinner{
		frames:   frames,
		interval: interval,
	}
}

func NewSpinner() *spinner {
	return NewCustomSpinner(Frames1, time.Millisecond*77)
}

// Stop stops the spinner.
func (s *spinner) Stop(clearLine bool) {
	if s.ticker != nil {
		s.ticker.Stop()
	}
	s.ticker = nil
	if clearLine {
		cursor.ClearLine()
		cursor.StartOfLine()
	} else {
		println()
	}
}

// start starts the spinner.
func (s *spinner) start() error {
	s.ticker = time.NewTicker(s.interval)
	go func() {
		for range s.ticker.C {
			s.index = (s.index + 1) % len(s.frames)
			cursor.StartOfLine()
			print(s.frames[s.index] + s.suffix)
		}
	}()
	return nil
}

// SetString sets the suffix of the spinner.
// The suffix is trimmed and the first line is used.
// Because the spinner is always on the same line, the suffix should not contain "\n".
func (s *spinner) SetString(suffix string) {
	if s.ticker == nil {
		defer s.start()
	}
	suffix = strings.TrimSpace(suffix)
	suffix = strings.Split(suffix, "\n")[0]
	s.suffix = " " + suffix
}