package state

import (
	"fmt"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/binchunk"
)

type lkClosure struct {
	proto  *binchunk.Prototype // lua closure
	goFunc GoFunction          // go closure
	upVals []*any
}

func newLuaClosure(proto *binchunk.Prototype) *lkClosure {
	c := &lkClosure{proto: proto}
	if nUpvals := len(proto.Upvalues); nUpvals > 0 {
		c.upVals = make([]*any, nUpvals)
	}
	return c
}

func newGoClosure(f GoFunction, nUpvals int) *lkClosure {
	c := &lkClosure{goFunc: f}
	if nUpvals > 0 {
		c.upVals = make([]*any, nUpvals)
	}
	return c
}

func (c *lkClosure) String() string {
	if c.goFunc != nil {
		return fmt.Sprintf("%p", c.goFunc)
	}
	return fmt.Sprintf("%p", c.proto)
}
