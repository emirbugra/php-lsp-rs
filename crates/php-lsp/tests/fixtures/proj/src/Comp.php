<?php
namespace App;

class S {
    public function t(User|Team $either, ?User $maybe, Box $b, User $u0) {
        $either->/*1*/;
        $maybe?->/*2*/;
        $x = new Box(new User);
        $x->get()->/*3*/;
        $x->value->/*4*/;
        $coll = new UserColl;
        $coll->first()->/*5*/;
        $f = fn(User $u) => $u;
        $f(new User)->/*6*/;
        $arr = $this->list_();
        $arr[0]->/*7*/;
        foreach ($arr as $i) { $i->/*8*/; }
        $m = array_map(fn($z) => $z->team(), $arr);
        $m[0]->/*9*/;
        $c2 = wrap($arr);
        $c2->first()->/*10*/;
        $c2->map(fn($p) => $p->/*11*/);
        $s = $this->maybeTeam();
        if ($s instanceof Team) { $s->/*12*/; }
        if ($s === null) { return; }
        $s->/*13*/;
    }
    /** @return User[] */
    function list_() { return []; }
    /** @return User|Team|null */
    function maybeTeam() { return null; }
}
