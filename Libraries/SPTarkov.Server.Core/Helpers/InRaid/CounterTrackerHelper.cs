using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Models.Common;

namespace SPTarkov.Server.Core.Helpers.InRaid;

[Injectable]
public class CounterTrackerHelper
{
    private Dictionary<MongoId, int> _maxCounts = new();
    private readonly Dictionary<MongoId, int> _trackedCounts = new();

    /// <summary>
    /// Add dictionary of keys and their matching limits to track
    /// </summary>
    /// <param name="maxCounts">Values to store</param>
    public void AddDataToTrack(Dictionary<MongoId, int> maxCounts)
    {
        _maxCounts = maxCounts;
    }

    /// <summary>
    /// Increment the counter for passed in key, get back value determining if max value passed
    /// </summary>
    /// <param name="key"></param>
    /// <param name="countToIncrementBy"></param>
    /// <returns>True = above max count</returns>
    public bool IncrementCount(MongoId key, int countToIncrementBy = 1)
    {
        // Not tracked, skip
        if (!_maxCounts.Any() || !_maxCounts.ContainsKey(key))
        {
            return false;
        }

        _trackedCounts.TryAdd(key, 0);
        _trackedCounts[key] += countToIncrementBy;

        return _trackedCounts[key] > _maxCounts[key];
    }

    /// <summary>
    /// Snapshot of the limits being tracked, for handing to the native loot generator
    /// </summary>
    public Dictionary<MongoId, int> GetMaxCounts()
    {
        return new Dictionary<MongoId, int>(_maxCounts);
    }

    /// <summary>
    /// Snapshot of the counts reached so far, for handing to the native loot generator
    /// </summary>
    public Dictionary<MongoId, int> GetTrackedCounts()
    {
        return new Dictionary<MongoId, int>(_trackedCounts);
    }

    /// <summary>
    /// Replace the counts reached so far with the ones the native loot generator counted
    /// </summary>
    /// <param name="counts">Values to store</param>
    public void SetTrackedCounts(Dictionary<MongoId, int> counts)
    {
        _trackedCounts.Clear();
        foreach (var (key, count) in counts)
        {
            _trackedCounts[key] = count;
        }
    }

    public void Clear()
    {
        _trackedCounts.Clear();
        _maxCounts.Clear();
    }
}
