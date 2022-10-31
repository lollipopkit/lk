package state

import (
	"fmt"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/binchunk"
)

type closure struct {
	proto  *binchunk.Prototype // lua closure
	goFunc GoFunction          // go closure
	upVals []*any
}

func newLuaClosure(proto *binchunk.Prototype) *closure {
	c := &closure{proto: proto}
	if nUpvals := len(proto.Upvalues); nUpvals > 0 {
		c.upVals = make([]*any, nUpvals)
	}
	return c
}

func newGoClosure(f GoFunction, nUpvals int) *closure {
	c := &closure{goFunc: f}
	if nUpvals > 0 {
		c.upVals = make([]*any, nUpvals)
	}
	return c
}

func (c *closure) String() string {
	if c.goFunc != nil {
		return fmt.Sprintf("GoFunc: %p", c.goFunc)
	}
	return fmt.Sprintf("LkFunc: %p", c.proto)
}
