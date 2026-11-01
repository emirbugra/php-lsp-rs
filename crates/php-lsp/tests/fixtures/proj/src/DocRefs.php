<?php
namespace App;

use App\CRepo as R;

/**
 * @property CItem|null $item
 * @property-read \App\CItem[] $items
 * @method static CRepo|null findIt(CItem $x, array{a: CService} $opts)
 * @mixin CService
 * @template T of CItem
 */
class DocHolder {
    /** @var array<int, CItem> */
    public $list;
    /** @var non-empty-string */
    public $s;

    /**
     * @param list<CItem> $a
     * @param R $r
     * @param T $t
     * @return \Illuminate\Support\Collection<int, CItem>|CService
     * @throws \RuntimeException
     * @see CRepo::find()
     */
    public function m($a, $r, $t) {
        /** @var CItem $x */
        $x = null;
        return $x;
    }
}
