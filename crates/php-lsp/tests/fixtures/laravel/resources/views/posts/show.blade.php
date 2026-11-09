<x-card :title="$post->title" class="p-4">
    {{ $related->author()->name }}
    @if ($errors->any())
        <span>{{ $errors->first() }}</span>
    @endif
</x-card>
