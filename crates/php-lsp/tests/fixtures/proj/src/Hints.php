<?php
namespace App;

class Hints
{
    private $count = 0;

    public function total($items)
    {
        $sum = 0;
        foreach ($items as $i) {
            $sum += 1;
        }
        return $sum;
    }

    /** @param int $limit */
    public function make($limit, ...$rest)
    {
        if ($limit > 1) {
            return new Sub\Gadget();
        }
        throw new \RuntimeException('no');
    }

    #[Deprecated]
    public function names(array $list)
    {
        return array_values($list);
    }
}
