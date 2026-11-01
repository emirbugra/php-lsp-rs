<?php
namespace App;

class User { public string $name = ''; public function posts(): array { return []; } public function team(): ?Team { return null; } }
class Team { public function members(): array { return []; } }

/**
 * @template T
 */
class Box {
    /** @var T */
    public $value;
    /** @param T $v */
    public function __construct($v) { $this->value = $v; }
    /** @return T */
    public function get() { return $this->value; }
    /** @return static */
    public function self() { return $this; }
}

/**
 * @template TKey of array-key
 * @template TValue
 */
class Coll implements \IteratorAggregate {
    /** @var array<TKey, TValue> */
    protected $items = [];
    /** @return TValue|null */
    public function first() { return null; }
    /** @return Coll<TKey, TValue> */
    public function filter(callable $cb = null) { return $this; }
    /**
     * @template U
     * @param callable(TValue): U $cb
     * @return Coll<TKey, U>
     */
    public function map(callable $cb) { return new Coll; }
    /** @return TValue[] */
    public function all() { return []; }
    public function getIterator(): \Traversable { return new \ArrayIterator([]); }
}

/**
 * @template T
 * @param class-string<T> $cls
 * @return T
 */
function make(string $cls) { return new $cls; }

/**
 * @template TValue
 * @param iterable<TValue> $items
 * @return Coll<int, TValue>
 */
function wrap($items) { return new Coll; }

/** @extends Coll<int, User> */
class UserColl extends Coll {}

class Svc {
    /** @var User[] */
    private array $users = [];
    /** @var list<Team> */
    public $teams;
    /** @return User[] */
    public function list(): array { return []; }
    /** @return array<string, User> */
    public function map(): array { return []; }
    public function run(?User $maybe, User|Team $either, $untyped) {
        $a = $this->list();
        $a; //@ App\User[]
        $first = $a[0];
        $first; //@ App\User
        foreach ($this->users as $u) {
            $u; //@ App\User
        }
        foreach ($this->map() as $k => $v) {
            $v; //@ App\User
        }
        foreach ($this->teams as $t) {
            $t; //@ App\Team
        }
        $lit = [new User, new User];
        $lit; //@ App\User[]
        foreach ($lit as $l) { $l; } //@ App\User
        [$p, $q] = $lit;
        $p; //@ App\User
        $arr = [];
        $arr[] = new Team;
        $arr; //@ App\Team[]
        $box = new Box(new User);
        $box; //@ App\Box<App\User>
        $box->get(); //@
        $got = $box->get();
        $got; //@ App\User
        $m = make(Team::class);
        $m; //@ App\Team
        $w = wrap($this->users);
        $w; //@ App\Coll<mixed,App\User>
        $f = $w->first();
        $f; //@ App\User
        $all = $w->all();
        $all; //@ App\User[]
        $uc = new UserColl;
        $uf = $uc->first();
        $uf; //@ App\User
        $mapped = $w->map(fn($x) => new Team);
        $mapped; //@ App\Coll<mixed,App\Team>
        foreach ($w as $it) { $it; } //@ App\User
        $filtered = array_filter($this->users);
        $filtered; //@ App\User[]
        $one = reset($this->users);
        $one; //@ App\User
        $mp = array_map(fn($u) => $u->team(), $this->users);
        $mp; //@ App\Team[]
        $w->each(function ($item) { $item; }); //@ App\User
        array_map(function ($u) { $u; }, $this->users); //@ App\User
        // narrowing
        if ($either instanceof User) {
            $either; //@ App\User
        } elseif ($either instanceof Team) {
            $either; //@ App\Team
        }
        if (!($untyped instanceof Team)) {
            return;
        }
        $untyped; //@ App\Team
        $z = $maybe instanceof User ? $maybe : new Team;
        $z; //@ App\User|App\Team
        assert($either instanceof Team);
        $either; //@ App\Team
    }
}
