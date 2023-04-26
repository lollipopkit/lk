package state

import . "github.com/lollipopkit/lk/api"

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#lua_newthread
// lua-5.3.4/src/lstate.c#lua_newthread()
func (self *lkState) NewThread() LkState {
	t := &lkState{registry: self.registry}
	t.pushLuaStack(newLuaStack(LK_MINSTACK, t))
	self.stack.push(t)
	return t
}

// [-?, +?, –]
// http://www.lua.org/manual/5.3/manual.html#lua_resume
func (self *lkState) Resume(from LkState, nArgs int) LkStatus {
	lsFrom := from.(*lkState)
	if lsFrom.coChan == nil {
		lsFrom.coChan = make(chan int)
	}

	if self.coChan == nil {
		// start coroutine
		self.coChan = make(chan int)
		self.coCaller = lsFrom
		go func() {
			self.coStatus = self.PCall(nArgs, -1, 0)
			lsFrom.coChan <- 1
		}()
	} else {
		// resume coroutine
		if self.coStatus != LK_YIELD { // todo
			self.stack.push("cannot resume non-suspended coroutine")
			return LK_ERRRUN
		}
		self.coStatus = LK_OK
		self.coChan <- 1
	}

	<-lsFrom.coChan // wait coroutine to finish or yield
	return self.coStatus
}

// [-?, +?, e]
// http://www.lua.org/manual/5.3/manual.html#lua_yield
func (self *lkState) Yield(nResults int) LkStatus {
	if self.coCaller == nil { // todo
		panic("attempt to yield from outside a coroutine")
	}
	self.coStatus = LK_YIELD
	self.coCaller.coChan <- 1
	<-self.coChan
	return LkStatus(self.GetTop())
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isyieldable
func (self *lkState) IsYieldable() bool {
	if self.isMainThread() {
		return false
	}
	return self.coStatus != LK_YIELD // todo
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_status
// lua-5.3.4/src/lapi.c#lua_status()
func (self *lkState) Status() LkStatus {
	return self.coStatus
}

// debug
func (self *lkState) GetStack() bool {
	return self.stack.prev != nil
}
