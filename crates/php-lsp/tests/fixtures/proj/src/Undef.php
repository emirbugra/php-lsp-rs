<?php
namespace App;

class U {
    public function bad($a) {
        echo $a;
        return $undef1; //@ BAD
    }
    public function good($items, $cb) {
        foreach ($items as $k => $v) { echo $k, $v; }
        foreach ($items as [$x, $y]) { echo $x, $y; }
        $arr[] = 1;
        $map['a']['b'] = 2;
        [$p, [$q]] = $items;
        list('k' => $r) = $items;
        preg_match('/a/', 'a', $m);
        echo $m[0], $p, $q, $r, $arr, $map;
        try { throw new \Exception(); } catch (\Exception $e) { echo $e; }
        global $gg; echo $gg;
        static $cache = []; echo $cache;
        $f = function ($z) use ($items, &$out) { $out = $z; return $items; };
        $g = fn($n) => $n + $items;
        if (isset($nope) || empty($nope2)) {}
        $d = $maybe ?? 1;
        $e2 = @$silent;
        $i++;
        echo compact('items');
        for ($j = 0; $j < 3; $j++) { echo $j; }
        while ($loop = array_pop($items)) { echo $loop; }
        echo $_GET['x'], $this->good(1, 2);
        $s = "val {$items} $cb";
        unset($gone);
        return $f . $g . $d . $e2 . $out;
    }
    public function dyn() { extract(['a' => 1]); return $a; }
    public function bad2() { $f = fn() => $nothere; return strlen($also); } //@ BAD x2
}
function top($p) { return $p . $zz; } //@ BAD
$global_code = $whatever;
